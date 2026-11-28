//! H.264 through this machine's backends and back. Each test skips, with the
//! reason printed, where the machine has no H.264 encoder or decoder (a Linux
//! box without OpenH264 or with one too old to decode, a Windows image
//! without Media Foundation's transforms), unless `DEVLANES_REQUIRE_H264`
//! names the missing side, and fails when a codec takes longer than
//! [`TIMEOUT`].

use std::fmt::Debug;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use devlanes_gui_codec::{
    CodecError, DecoderBackend, EncodeParams, Encoded, EncoderBackend, Frame, Packet, VideoDecoder,
    h264,
};
use devlanes_gui_host::pattern::{draw_grid, shows_grid_within};
use devlanes_gui_proto::{REFINE_QUALITY_STEP, Size};

/// How long one test may take with a real codec.
const TIMEOUT: Duration = Duration::from_secs(120);

/// How far a channel of a grid cell's centre may be off after H.264: flat
/// areas come back close to exact, so more means a colour conversion bug.
const GRID_TOLERANCE: u8 = 8;

/// Whether `frame` shows the grid within [`GRID_TOLERANCE`].
fn shows_grid(frame: &Frame) -> bool {
    shows_grid_within(frame, GRID_TOLERANCE)
}

/// Skips for a missing `side` (`encode` or `decode`), printing why, or fails
/// where CI set `DEVLANES_REQUIRE_H264` to a list naming it, such as
/// `encode,decode`, since the codec must be there.
fn skip(side: &str, why: impl Debug) {
    let required = std::env::var("DEVLANES_REQUIRE_H264").unwrap_or_default();
    assert!(
        !required.split(',').any(|s| s.trim() == side),
        "H.264 {side} is required here: {why:?}"
    );
    eprintln!("skipped: no H.264 {side} here: {why:?}");
}

/// The committed stream: three length-prefixed access units of the grid at
/// 160x90, a keyframe with its parameter sets and two predicted frames.
fn fixture() -> Vec<&'static [u8]> {
    let mut rest = &include_bytes!("../fixtures/grid-160x90.h264")[..];
    let mut units = Vec::new();
    while let Some((length, after)) = rest.split_first_chunk::<4>() {
        let (unit, next) = after.split_at(u32::from_be_bytes(*length) as usize);
        units.push(unit);
        rest = next;
    }
    units
}

/// Runs `test` on a thread of its own, failing when it takes longer than
/// [`TIMEOUT`]: an OS codec can hang in a driver.
fn bounded(test: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        test();
        let _ = done.send(());
    });
    match finished.recv_timeout(TIMEOUT) {
        Ok(()) => thread.join().expect("the test thread"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // The test panicked; its panic is the failure.
            if let Err(panic) = thread.join() {
                std::panic::resume_unwind(panic);
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("the codec took more than {TIMEOUT:?}"),
    }
}

/// Every encoder of this machine with its decoder, or none with the reason
/// printed.
fn codecs() -> Vec<(Arc<dyn EncoderBackend>, Arc<dyn DecoderBackend>)> {
    let found = h264();
    if found.encoders.is_empty() {
        skip("encode", &found.encode);
    }
    let Some(decoder) = found.decoders.first().cloned() else {
        skip("decode", &found.decode);
        return Vec::new();
    };
    found
        .encoders
        .into_iter()
        .map(|encoder| (encoder, decoder.clone()))
        .collect()
}

/// What the pacer would ask of a frame at its starting rate.
fn params(keyframe: bool) -> EncodeParams {
    EncodeParams {
        bitrate_bps: 4_000_000,
        frame_interval: Duration::from_millis(33),
        quality: 60,
        keyframe,
        refine: false,
    }
}

/// The test pattern's colour grid at `width` by `height`.
fn grid(width: u32, height: u32) -> Frame {
    Frame::packed(width, height, draw_grid(Size::new(width, height))).expect("grid")
}

/// Decodes `encoded`, which must give a picture.
fn decode(decoder: &mut dyn VideoDecoder, encoded: &Encoded) -> Frame {
    decoder
        .decode(&Packet {
            payload: &encoded.payload,
            width: encoded.width,
            height: encoded.height,
            keyframe: encoded.keyframe,
            timestamp: Duration::ZERO,
        })
        .expect("decodes")
        .expect("a picture")
}

/// The grid comes back recognisable at every size, an odd one included
/// (padded to even sides by the encoder, cropped back by the decoder), and a
/// change of size restarts the stream with a keyframe that the same decoder
/// follows.
#[test]
fn the_grid_survives_h264_at_any_size_and_after_a_resize() {
    for (encoders, decoders) in codecs() {
        bounded(move || {
            let mut encoder = encoders.encoder().expect("an encoder");
            let mut decoder = decoders.decoder().expect("a decoder");
            for (width, height) in [(640, 400), (321, 199), (800, 600)] {
                let frame = grid(width, height);
                let first = encoder
                    .encode(&frame, &params(false))
                    .expect("encodes")
                    .expect("a frame");
                assert!(first.keyframe, "a new size starts with a keyframe");
                assert_eq!((first.width, first.height), (width, height));
                let next = encoder
                    .encode(&frame, &params(false))
                    .expect("encodes")
                    .expect("a frame");
                for encoded in [first, next] {
                    let picture = decode(&mut *decoder, &encoded);
                    assert_eq!((picture.width(), picture.height()), (width, height));
                    assert!(
                        shows_grid(&picture),
                        "{width}x{height} from {:?}",
                        encoders.offer()
                    );
                }
            }
        });
    }
}

/// A keyframe asked for in mid-stream decodes on a decoder that saw nothing
/// before it, which is how a viewer recovers and how one joins.
#[test]
fn a_requested_keyframe_decodes_on_its_own() {
    for (encoders, decoders) in codecs() {
        bounded(move || {
            let mut encoder = encoders.encoder().expect("an encoder");
            let frame = grid(480, 270);
            for _ in 0..3 {
                encoder.encode(&frame, &params(false)).expect("encodes");
            }
            let key = encoder
                .encode(&frame, &params(true))
                .expect("encodes")
                .expect("a keyframe is never skipped");
            assert!(key.keyframe);
            let mut fresh = decoders.decoder().expect("a decoder");
            assert!(shows_grid(&decode(&mut *fresh, &key)));
        });
    }
}

/// Every encoder starts and gives a keyframe of the grid. Where OpenH264 is
/// too old to decode (Ubuntu 24.04's 2.4), this is the encoders' only test.
#[test]
fn every_encoder_gives_a_keyframe() {
    let found = h264();
    if found.encoders.is_empty() {
        skip("encode", &found.encode);
    }
    for encoders in found.encoders {
        bounded(move || {
            let mut encoder = encoders.encoder().expect("an encoder");
            let key = encoder
                .encode(&grid(320, 180), &params(true))
                .expect("encodes")
                .expect("a keyframe is never skipped");
            assert!(
                key.keyframe && !key.payload.is_empty(),
                "{:?}",
                encoders.offer()
            );
            assert_eq!((key.width, key.height), (320, 180));
        });
    }
}

/// The committed stream (`examples/h264_fixture.rs` wrote it from the test
/// pattern with Microsoft's encoder; written again on another OS, its bytes
/// differ) decodes to the grid with every decoder of this machine. It is the
/// macOS decoder's only test of pictures, since nothing on a Mac encodes
/// H.264 yet, and it shows every decoder reads another encoder's stream.
#[test]
fn the_committed_stream_decodes_to_the_grid() {
    let found = h264();
    if found.decoders.is_empty() {
        skip("decode", &found.decode);
    }
    for decoders in found.decoders {
        bounded(move || {
            let mut decoder = decoders.decoder().expect("a decoder");
            let units = fixture();
            for (index, unit) in units.iter().enumerate() {
                let picture = decoder
                    .decode(&packet(unit, index == 0))
                    .expect("decodes")
                    .expect("a picture");
                assert!(
                    shows_grid(&picture),
                    "picture {index} from {:?}",
                    decoders.offer()
                );
            }
            assert_eq!(units.len(), 3);
        });
    }
}

/// A packet of the fixture's size.
fn packet(payload: &[u8], keyframe: bool) -> Packet<'_> {
    Packet {
        payload,
        width: 160,
        height: 90,
        keyframe,
        timestamp: Duration::ZERO,
    }
}

/// The NAL units of an Annex B access unit, without their start codes.
fn nal_units(access_unit: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= access_unit.len() {
        if access_unit[i..i + 3] == [0, 0, 1] {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(n, &start)| {
            let end = starts
                .get(n + 1)
                .map_or(access_unit.len(), |&next| next - 3);
            // A four-byte start code leaves a zero before the next one.
            let end = if end > start && access_unit[end - 1] == 0 && n + 1 < starts.len() {
                end - 1
            } else {
                end
            };
            &access_unit[start..end]
        })
        .collect()
}

/// Writes bits of an H.264 RBSP, with the emulation prevention bytes the
/// format needs.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    current: u8,
    used: u8,
}

impl Bits {
    /// Appends the low `count` bits of `value`, most significant first.
    fn put(&mut self, value: u32, count: u8) {
        for bit in (0..count).rev() {
            self.current = self.current << 1 | ((value >> bit) & 1) as u8;
            self.used += 1;
            if self.used == 8 {
                let zeros = self.bytes.ends_with(&[0, 0]);
                if zeros && self.current <= 3 {
                    self.bytes.push(3);
                }
                self.bytes.push(self.current);
                (self.current, self.used) = (0, 0);
            }
        }
    }

    /// Appends `value` as unsigned Exp-Golomb.
    fn ue(&mut self, value: u32) {
        let coded = value + 1;
        let length = 32 - coded.leading_zeros() as u8;
        self.put(0, length - 1);
        self.put(coded, length);
    }

    /// The bytes, ended with the RBSP stop bit.
    fn finish(mut self) -> Vec<u8> {
        self.put(1, 1);
        while self.used != 0 {
            self.put(0, 1);
        }
        self.bytes
    }
}

/// A Baseline sequence parameter set for a picture of 16384x16384, twice
/// the largest side devlanes takes.
fn huge_sps() -> Vec<u8> {
    let mut bits = Bits::default();
    bits.put(0x67, 8); // NAL header: SPS.
    bits.put(66, 8); // Baseline.
    bits.put(0, 8); // Constraint flags.
    bits.put(52, 8); // Level 5.2.
    bits.ue(0); // seq_parameter_set_id
    bits.ue(0); // log2_max_frame_num_minus4
    bits.ue(2); // pic_order_cnt_type
    bits.ue(1); // max_num_ref_frames
    bits.put(0, 1); // gaps_in_frame_num_value_allowed_flag
    bits.ue(1023); // pic_width_in_mbs_minus1
    bits.ue(1023); // pic_height_in_map_units_minus1
    bits.put(1, 1); // frame_mbs_only_flag
    bits.put(1, 1); // direct_8x8_inference_flag
    bits.put(0, 1); // frame_cropping_flag
    bits.put(0, 1); // vui_parameters_present_flag
    bits.finish()
}

/// Bytes from a xorshift generator, the same on every run.
fn garbage(length: usize, mut seed: u32) -> Vec<u8> {
    (0..length)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect()
}

/// Feeds `payload` to `decoder`: an error is fine, a panic or a hang is not,
/// and a picture must have the packet's size.
fn feed(decoder: &mut dyn VideoDecoder, payload: &[u8], what: &str) {
    if let Ok(Some(picture)) = decoder.decode(&packet(payload, true)) {
        assert_eq!((picture.width(), picture.height()), (160, 90), "{what}");
    }
}

/// Every decoder survives what a broken or hostile host could send, seeded
/// from the committed stream: garbage with and without start codes, the
/// stream cut short every few bytes, a predicted frame before any parameter
/// set, a parameter set for a picture past devlanes' limit and an empty
/// payload. Afterwards a fresh decoder still decodes the stream.
#[test]
fn every_decoder_survives_broken_input() {
    let found = h264();
    if found.decoders.is_empty() {
        skip("decode", &found.decode);
    }
    for decoders in found.decoders {
        bounded(move || {
            let units = fixture();
            let offer = decoders.offer();
            let fresh = || decoders.decoder().expect("a decoder");

            let mut decoder = fresh();
            for (n, length) in [1, 3, 17, 300, 4096].into_iter().enumerate() {
                let noise = garbage(length, 0x9e37_79b9 ^ n as u32);
                feed(&mut *decoder, &noise, "garbage");
                let mut coded = vec![0, 0, 0, 1];
                coded.extend_from_slice(&noise);
                feed(&mut *decoder, &coded, "garbage after a start code");
            }

            let mut decoder = fresh();
            for cut in (0..units[0].len()).step_by(5) {
                feed(&mut *decoder, &units[0][..cut], "a cut keyframe");
            }

            let mut decoder = fresh();
            feed(&mut *decoder, units[1], "a predicted frame first");

            let nals = nal_units(units[0]);
            let mut huge = Vec::new();
            for unit in nals {
                huge.extend_from_slice(&[0, 0, 0, 1]);
                match unit[0] & 0x1f {
                    7 => huge.extend_from_slice(&huge_sps()),
                    _ => huge.extend_from_slice(unit),
                }
            }
            let mut decoder = fresh();
            feed(&mut *decoder, &huge, "a 16384x16384 parameter set");

            let mut decoder = fresh();
            assert!(
                matches!(
                    decoder.decode(&packet(&[], true)),
                    Err(CodecError::BadPayload { .. })
                ),
                "an empty payload from {offer:?}"
            );

            let mut decoder = fresh();
            for (index, unit) in units.iter().enumerate() {
                let picture = decoder.decode(&packet(unit, index == 0));
                let picture = picture.expect("decodes").expect("a picture");
                assert!(shows_grid(&picture), "{offer:?} after broken input");
            }
        });
    }
}

/// Rows of fine coloured strokes on white, like small text, which 4:2:0 at a
/// low bitrate blurs.
fn text(width: u32, height: u32) -> Frame {
    let mut pixels = vec![0xffu8; width as usize * height as usize * 4];
    let mut seed = 0x2545_f491_u32;
    for y in 0..height {
        for x in 0..width {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            // Glyph-sized cells with a line of space between rows of text.
            if y % 14 < 11 && x % 8 < 6 && seed.is_multiple_of(3) {
                let at = (y as usize * width as usize + x as usize) * 4;
                let colour = match (x / 64 + y / 14) % 3 {
                    0 => [0x20, 0x20, 0x20],
                    1 => [0xc0, 0x30, 0x20],
                    _ => [0x20, 0x60, 0xd0],
                };
                pixels[at..at + 3].copy_from_slice(&colour);
            }
        }
    }
    Frame::packed(width, height, pixels).expect("text")
}

/// The mean difference in brightness between two pictures of one size. The
/// colour of single pixels is lost to 4:2:0 at any quality, so edges, which
/// make text readable, are what a refinement can bring back.
fn distance(a: &Frame, b: &Frame) -> f64 {
    let (a, b) = (a.to_packed(), b.to_packed());
    let luma = |px: &[u8; 4]| {
        0.0722 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.2126 * f64::from(px[2])
    };
    let total: f64 = a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .map(|(x, y)| (luma(x) - luma(y)).abs())
        .sum();
    total / (a.len() / 4) as f64
}

/// After the picture stops changing, the refinement of the same picture, at
/// the quality the pacer asks for it, is closer to it than the last frame at
/// the low rate was, comes as a predicted frame (an IDR would make every
/// viewer start over) and costs no more than the stream's keyframe did.
#[test]
fn a_refinement_sharpens_a_still_picture() {
    for (encoders, decoders) in codecs() {
        bounded(move || {
            let mut encoder = encoders.encoder().expect("an encoder");
            let mut decoder = decoders.decoder().expect("a decoder");
            let picture = text(640, 360);
            // What the pacer asks on a slow link.
            let slow = EncodeParams {
                bitrate_bps: 300_000,
                quality: 20,
                ..params(false)
            };
            let (mut last, mut keyframe_bytes) = (None, 0);
            for _ in 0..5 {
                if let Some(encoded) = encoder.encode(&picture, &slow).expect("encodes") {
                    if encoded.keyframe {
                        keyframe_bytes = encoded.payload.len();
                    }
                    last = Some(decode(&mut *decoder, &encoded));
                }
            }
            let before = distance(&picture, &last.expect("a frame at the low rate"));
            let refine = EncodeParams {
                refine: true,
                quality: slow.quality + REFINE_QUALITY_STEP,
                ..slow
            };
            let refined = encoder
                .encode(&picture, &refine)
                .expect("encodes")
                .expect("a refinement");
            let offer = encoders.offer();
            assert!(
                !refined.keyframe,
                "a refinement is no keyframe from {offer:?}"
            );
            assert!(
                refined.payload.len() <= keyframe_bytes,
                "a refinement of {} bytes after a {keyframe_bytes}-byte keyframe from {offer:?}",
                refined.payload.len()
            );
            let after = distance(&picture, &decode(&mut *decoder, &refined));
            eprintln!(
                "{offer:?}: {before:.2} to {after:.2} in {} bytes",
                refined.payload.len()
            );
            // A predicted frame a few steps finer sharpens a little; a large
            // gain would take a large frame, which the bound rules out.
            assert!(
                after < before,
                "refined {after:.2} against {before:.2} from {offer:?}"
            );
        });
    }
}
