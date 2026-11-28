//! Colour conversion between BGRA frames and the 4:2:0 layouts H.264 codecs
//! take: I420 (three planes, OpenH264) and NV12 (luma and interleaved
//! chroma, Media Foundation and VideoToolbox).
//!
//! Every devlanes encoder and decoder converts here, with one matrix (BT.709,
//! limited range, which the OpenH264 encoder also signals in the stream), so
//! a picture keeps its colours whichever two backends meet. Integer
//! arithmetic over two rows at a time: each 2x2 block yields four luma
//! samples and one chroma pair from the average of its four pixels.
//!
//! H.264 4:2:0 needs even sides. A frame with an odd side is padded here by
//! repeating its last column or row, the encoded picture has the even size,
//! and the decoder crops back to the size the frame message carries.

use crate::Frame;

/// `n` rounded up to even.
pub(crate) fn even(n: u32) -> u32 {
    n + (n & 1)
}

/// Luma of one pixel.
#[inline(always)]
fn luma(b: i32, g: i32, r: i32) -> u8 {
    (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8
}

/// The chroma pair of a block from the sums of its four pixels' channels.
#[inline(always)]
fn chroma(b4: i32, g4: i32, r4: i32) -> (u8, u8) {
    // Each row of coefficients sums to zero, so greys have no colour.
    let u = ((-26 * r4 - 86 * g4 + 112 * b4 + 512) >> 10) + 128;
    let v = ((112 * r4 - 102 * g4 - 10 * b4 + 512) >> 10) + 128;
    (u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
}

/// The blue, green and red terms of one chroma pair, shared by the pixels
/// of its block, rounding included.
#[inline(always)]
fn chroma_terms(u: u8, v: u8) -> (i32, i32, i32) {
    let (u, v) = (i32::from(u) - 128, i32::from(v) - 128);
    (541 * u + 128, -55 * u - 136 * v + 128, 459 * v + 128)
}

/// Writes one pixel of BGRA from its luma and its block's chroma terms.
#[inline(always)]
fn pixel(out: &mut [u8], y: u8, (b, g, r): (i32, i32, i32)) {
    let y = 298 * (i32::from(y) - 16);
    let clamp = |value: i32| (value >> 8).clamp(0, 255) as u8;
    out[0] = clamp(y + b);
    out[1] = clamp(y + g);
    out[2] = clamp(y + r);
    out[3] = 0xff;
}

/// Converts one output row from its luma row and the chroma pair of each
/// two pixels, which `chroma` gives by pair index.
#[inline(always)]
fn row_to_bgra(line: &mut [u8], y: &[u8], chroma: impl Fn(usize) -> (u8, u8)) {
    let width = y.len();
    let pairs = width / 2;
    let (outs, _) = line[..pairs * 8].as_chunks_mut::<8>();
    let (lumas, _) = y[..pairs * 2].as_chunks::<2>();
    for (i, (out, luma)) in outs.iter_mut().zip(lumas).enumerate() {
        let (u, v) = chroma(i);
        let terms = chroma_terms(u, v);
        pixel(&mut out[..4], luma[0], terms);
        pixel(&mut out[4..], luma[1], terms);
    }
    if !width.is_multiple_of(2) {
        let (u, v) = chroma(pairs);
        pixel(&mut line[pairs * 8..], y[width - 1], chroma_terms(u, v));
    }
}

/// One row pair of a frame: the two source rows (the same row twice for the
/// last row of an odd height), and where its luma and chroma go.
struct RowPair<'a> {
    top: &'a [u8],
    bottom: &'a [u8],
    width: usize,
}

impl RowPair<'_> {
    /// Converts the pair, calling `chroma_out` with each block's index and
    /// chroma pair and filling the two luma rows.
    #[inline(always)]
    fn convert(
        &self,
        y_top: &mut [u8],
        y_bottom: &mut [u8],
        mut chroma_out: impl FnMut(usize, u8, u8),
    ) {
        let pairs = self.width / 2;
        let (top, _) = self.top[..pairs * 8].as_chunks::<8>();
        let (bottom, _) = self.bottom[..pairs * 8].as_chunks::<8>();
        let (y_top_pairs, _) = y_top[..pairs * 2].as_chunks_mut::<2>();
        let (y_bottom_pairs, _) = y_bottom[..pairs * 2].as_chunks_mut::<2>();
        for (i, (((t, b), yt), yb)) in top
            .iter()
            .zip(bottom)
            .zip(y_top_pairs.iter_mut())
            .zip(y_bottom_pairs.iter_mut())
            .enumerate()
        {
            let px = |s: &[u8; 8], at: usize| {
                (i32::from(s[at]), i32::from(s[at + 1]), i32::from(s[at + 2]))
            };
            let (b0, g0, r0) = px(t, 0);
            let (b1, g1, r1) = px(t, 4);
            let (b2, g2, r2) = px(b, 0);
            let (b3, g3, r3) = px(b, 4);
            yt[0] = luma(b0, g0, r0);
            yt[1] = luma(b1, g1, r1);
            yb[0] = luma(b2, g2, r2);
            yb[1] = luma(b3, g3, r3);
            let (u, v) = chroma(b0 + b1 + b2 + b3, g0 + g1 + g2 + g3, r0 + r1 + r2 + r3);
            chroma_out(i, u, v);
        }
        if !self.width.is_multiple_of(2) {
            // The last column, repeated into the padding column.
            let at = (self.width - 1) * 4;
            let px = |s: &[u8]| (i32::from(s[at]), i32::from(s[at + 1]), i32::from(s[at + 2]));
            let (b0, g0, r0) = px(self.top);
            let (b2, g2, r2) = px(self.bottom);
            let (top_luma, bottom_luma) = (luma(b0, g0, r0), luma(b2, g2, r2));
            y_top[pairs * 2] = top_luma;
            y_top[pairs * 2 + 1] = top_luma;
            y_bottom[pairs * 2] = bottom_luma;
            y_bottom[pairs * 2 + 1] = bottom_luma;
            let (u, v) = chroma(2 * (b0 + b2), 2 * (g0 + g2), 2 * (r0 + r2));
            chroma_out(pairs, u, v);
        }
    }
}

/// The row pairs of `frame`, with the index of each.
fn row_pairs(frame: &Frame) -> impl Iterator<Item = (usize, RowPair<'_>)> {
    let height = frame.height();
    (0..even(height) / 2).map(move |j| {
        let top = 2 * j;
        let bottom = (top + 1).min(height - 1);
        let row = |y| frame.row(y).unwrap_or_default();
        (
            j as usize,
            RowPair {
                top: row(top),
                bottom: row(bottom),
                width: frame.width() as usize,
            },
        )
    })
}

/// A picture in I420 at a frame's even size, reused from frame to frame.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Default)]
pub(crate) struct I420Buffer {
    /// Even width.
    pub width: u32,
    /// Even height.
    pub height: u32,
    /// Luma, `width` bytes a row.
    pub y: Vec<u8>,
    /// Blue difference, `width / 2` bytes a row.
    pub u: Vec<u8>,
    /// Red difference, `width / 2` bytes a row.
    pub v: Vec<u8>,
}

#[cfg(any(target_os = "linux", test))]
impl I420Buffer {
    /// Converts `frame`, padded to even sides.
    pub fn fill(&mut self, frame: &Frame) {
        let (width, height) = (even(frame.width()), even(frame.height()));
        let (w, cw) = (width as usize, width as usize / 2);
        self.width = width;
        self.height = height;
        self.y.resize(w * height as usize, 0);
        self.u.resize(cw * height as usize / 2, 0);
        self.v.resize(cw * height as usize / 2, 0);
        let y_rows = self.y.chunks_exact_mut(2 * w);
        let chroma_rows = self.u.chunks_exact_mut(cw).zip(self.v.chunks_exact_mut(cw));
        for (((_, pair), y_rows), (u_row, v_row)) in row_pairs(frame).zip(y_rows).zip(chroma_rows) {
            let (top, bottom) = y_rows.split_at_mut(w);
            pair.convert(top, bottom, |i, u, v| {
                u_row[i] = u;
                v_row[i] = v;
            });
        }
    }
}

/// Converts `frame`, padded to even sides, into NV12 in `out`: the luma rows
/// `stride` bytes apart, then the interleaved chroma rows after
/// `chroma_offset` bytes, also `stride` apart.
#[cfg(any(windows, test))]
pub(crate) fn to_nv12(frame: &Frame, out: &mut [u8], stride: usize, chroma_offset: usize) {
    let (luma_plane, chroma_plane) = out.split_at_mut(chroma_offset);
    let y_rows = luma_plane.chunks_mut(2 * stride);
    let chroma_rows = chroma_plane.chunks_mut(stride);
    for (((_, pair), y_rows), uv_row) in row_pairs(frame).zip(y_rows).zip(chroma_rows) {
        let (top, bottom) = y_rows.split_at_mut(stride);
        pair.convert(top, bottom, |i, u, v| {
            uv_row[2 * i] = u;
            uv_row[2 * i + 1] = v;
        });
    }
}

/// The planes of a decoded 4:2:0 picture.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Planes<'a> {
    /// Separate chroma planes.
    #[cfg(any(target_os = "linux", test))]
    I420 {
        y: &'a [u8],
        u: &'a [u8],
        v: &'a [u8],
        y_stride: usize,
        uv_stride: usize,
    },
    /// Interleaved chroma, blue difference first.
    #[cfg(any(windows, target_os = "macos", test))]
    Nv12 {
        y: &'a [u8],
        uv: &'a [u8],
        y_stride: usize,
        uv_stride: usize,
    },
}

/// Converts the top-left `width` by `height` of a decoded picture to packed
/// BGRA. The caller checks the planes hold that much.
pub(crate) fn to_bgra(planes: Planes<'_>, width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h * 4];
    for (row, line) in out.chunks_exact_mut(w * 4).enumerate() {
        match planes {
            #[cfg(any(target_os = "linux", test))]
            Planes::I420 {
                y,
                u,
                v,
                y_stride,
                uv_stride,
            } => {
                let y = &y[row * y_stride..][..w];
                let c = (row / 2) * uv_stride;
                let (u, v) = (&u[c..][..w.div_ceil(2)], &v[c..][..w.div_ceil(2)]);
                row_to_bgra(line, y, |i| (u[i], v[i]));
            }
            #[cfg(any(windows, target_os = "macos", test))]
            Planes::Nv12 {
                y,
                uv,
                y_stride,
                uv_stride,
            } => {
                let y = &y[row * y_stride..][..w];
                let uv = &uv[(row / 2) * uv_stride..][..2 * w.div_ceil(2)];
                row_to_bgra(line, y, |i| (uv[2 * i], uv[2 * i + 1]));
            }
        }
    }
    out
}

/// Whether `len` bytes at `stride` hold `rows` rows of `width` bytes.
pub(crate) fn holds(len: usize, stride: usize, width: usize, rows: usize) -> bool {
    rows == 0
        || (stride >= width
            && stride
                .checked_mul(rows - 1)
                .and_then(|before| before.checked_add(width))
                .is_some_and(|needed| len >= needed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of `width` by `height` filled with BGRA `pixel`.
    fn solid(width: u32, height: u32, pixel: [u8; 4]) -> Frame {
        let data: Vec<u8> = (0..width * height).flat_map(|_| pixel).collect();
        Frame::packed(width, height, data).expect("frame")
    }

    /// Solid colours survive the round trip within rounding, white and black
    /// map to the ends of the limited range, and both layouts agree.
    #[test]
    fn colours_round_trip_through_both_layouts() {
        let white = solid(4, 2, [255, 255, 255, 255]);
        let mut i420 = I420Buffer::default();
        i420.fill(&white);
        assert!(i420.y.iter().all(|&y| y == 235), "{:?}", i420.y);
        assert!(i420.u.iter().chain(&i420.v).all(|&c| c == 128));
        let black = solid(4, 2, [0, 0, 0, 255]);
        i420.fill(&black);
        assert!(i420.y.iter().all(|&y| y == 16));

        for colour in [
            [0x4b, 0x19, 0xe6, 0xff],
            [0x4b, 0xb4, 0x3c, 0xff],
            [0xd8, 0x63, 0x43, 0xff],
        ] {
            let frame = solid(6, 4, colour);
            i420.fill(&frame);
            let from_i420 = to_bgra(
                Planes::I420 {
                    y: &i420.y,
                    u: &i420.u,
                    v: &i420.v,
                    y_stride: 6,
                    uv_stride: 3,
                },
                6,
                4,
            );
            let mut nv12 = vec![0; 6 * 4 * 3 / 2];
            to_nv12(&frame, &mut nv12, 6, 24);
            let from_nv12 = to_bgra(
                Planes::Nv12 {
                    y: &nv12[..24],
                    uv: &nv12[24..],
                    y_stride: 6,
                    uv_stride: 6,
                },
                6,
                4,
            );
            assert_eq!(from_i420, from_nv12);
            for px in from_i420.as_chunks::<4>().0 {
                for c in 0..3 {
                    assert!(px[c].abs_diff(colour[c]) <= 3, "{px:?} for {colour:?}");
                }
            }
        }
    }

    /// An odd side is padded by repeating the last column and row, so the
    /// padding never brings a colour the picture does not have.
    #[test]
    fn odd_sides_are_padded_with_the_last_column_and_row() {
        // 3x3: a red last column and a blue last row, green elsewhere.
        let (red, green, blue) = ([0, 0, 255, 255], [0, 255, 0, 255], [255, 0, 0, 255]);
        let data: Vec<u8> = (0..9)
            .flat_map(|i| match (i % 3, i / 3) {
                (_, 2) => blue,
                (2, _) => red,
                _ => green,
            })
            .collect();
        let frame = Frame::packed(3, 3, data).expect("frame");
        let mut i420 = I420Buffer::default();
        i420.fill(&frame);
        assert_eq!((i420.width, i420.height), (4, 4));
        let luma = |b: i32, g: i32, r: i32| super::luma(b, g, r);
        assert_eq!(i420.y[3], luma(0, 0, 255), "padding column repeats red");
        assert_eq!(i420.y[3 * 4], luma(255, 0, 0), "padding row repeats blue");
        assert_eq!(i420.y[3 * 4 + 3], luma(255, 0, 0));
        let back = to_bgra(
            Planes::I420 {
                y: &i420.y,
                u: &i420.u,
                v: &i420.v,
                y_stride: 4,
                uv_stride: 2,
            },
            3,
            3,
        );
        assert_eq!(back.len(), 3 * 3 * 4, "cropped to the frame's size");
    }

    /// Prints the time of each conversion per frame; the spike's conversion
    /// took 9.3 ms for 1440x900.
    #[test]
    #[ignore = "a measurement: run it in release with --ignored --nocapture"]
    fn conversion_time() {
        for (width, height) in [(1280u32, 800u32), (1440, 900), (1920, 1080)] {
            let data: Vec<u8> = (0..width * height * 4)
                .map(|i| (i * 7 % 251) as u8)
                .collect();
            let frame = Frame::packed(width, height, data).expect("frame");
            let rounds = 50;
            let (w, h) = (width as usize, height as usize);
            let mut i420 = I420Buffer::default();
            let start = std::time::Instant::now();
            for _ in 0..rounds {
                i420.fill(&frame);
            }
            let to_i420 = start.elapsed() / rounds;
            let mut nv12 = vec![0; w * h * 3 / 2];
            let start = std::time::Instant::now();
            for _ in 0..rounds {
                to_nv12(&frame, &mut nv12, w, w * h);
            }
            let to_nv = start.elapsed() / rounds;
            let start = std::time::Instant::now();
            for _ in 0..rounds {
                let (y, uv) = nv12.split_at(w * h);
                std::hint::black_box(to_bgra(
                    Planes::Nv12 {
                        y,
                        uv,
                        y_stride: w,
                        uv_stride: w,
                    },
                    width,
                    height,
                ));
            }
            let back = start.elapsed() / rounds;
            println!(
                "{width}x{height}: to I420 {to_i420:?}, to NV12 {to_nv:?}, NV12 to BGRA {back:?}"
            );
        }
    }

    /// Planes that are too short for what they claim are told apart.
    #[test]
    fn plane_sizes_are_checked() {
        assert!(holds(10, 4, 2, 3));
        assert!(!holds(9, 4, 2, 3));
        assert!(!holds(100, 1, 2, 3));
        assert!(holds(0, 0, 0, 0));
    }
}
