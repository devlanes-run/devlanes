//! Turns a frame and a lock state into the pixels of a local window: the
//! frame at its top-left corner, and for a locked window the frame dimmed
//! with one line of text. That overlay is the only thing devlanes draws.

use devlanes_gui_codec::Frame;

/// What fills the window outside the frame, and the window before a frame came.
const BACKGROUND: u32 = 0x00_20_20_20;

/// The overlay's text colour.
const TEXT: u32 = 0x00_f0_f0_f0;

/// Rows of a glyph, each the low five bits, the leftmost pixel highest.
type Glyph = [u8; 7];

/// The glyph of `c`, for the letters, digits and punctuation the overlay
/// lines use; anything else is blank.
fn glyph(c: char) -> Glyph {
    match c {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f],
        'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0c],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        ':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        _ => [0; 7],
    }
}

/// Paints `frame` into `buffer`, a `width` by `height` window of 0RGB
/// pixels, dimmed under `text` when the window is locked. `scale` sizes the
/// text for the screen's density.
pub fn paint(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    frame: Option<&Frame>,
    text: Option<&str>,
    scale: u32,
) {
    let (width, height) = (width as usize, height as usize);
    buffer.fill(BACKGROUND);
    if let Some(frame) = frame {
        let columns = width.min(frame.width() as usize);
        for y in 0..height.min(frame.height() as usize) {
            let Some(row) = frame.row(y as u32) else {
                break;
            };
            let out = &mut buffer[y * width..y * width + columns];
            for (pixel, bgra) in out.iter_mut().zip(row.as_chunks::<4>().0) {
                *pixel = u32::from(bgra[2]) << 16 | u32::from(bgra[1]) << 8 | u32::from(bgra[0]);
            }
        }
    }
    let Some(text) = text else {
        return;
    };
    for pixel in buffer.iter_mut() {
        // Each channel at two fifths, so the text reads on any picture.
        let dim = |shift: u32| ((*pixel >> shift & 0xff) * 2 / 5) << shift;
        *pixel = dim(16) | dim(8) | dim(0);
    }
    let scale = scale.max(1) as usize;
    let advance = 6 * scale;
    let line = text.chars().count() * advance;
    let left = width.saturating_sub(line) / 2;
    let top = height.saturating_sub(7 * scale) / 2;
    for (index, c) in text.chars().enumerate() {
        for (row, bits) in glyph(c).iter().enumerate() {
            for column in 0..5 {
                if bits & (0x10 >> column) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    let y = top + row * scale + dy;
                    let x0 = left + index * advance + column * scale;
                    if y >= height || x0 >= width {
                        continue;
                    }
                    let end = (x0 + scale).min(width);
                    buffer[y * width + x0..y * width + end].fill(TEXT);
                }
            }
        }
    }
}
