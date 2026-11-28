//! Times H.264 on this machine: the colour conversion, and encode and decode
//! per frame with each encoder found, on the test pattern and on text-like
//! content, at 1280x800 and 1920x1080. Run it in release:
//!
//! ```sh
//! cargo run --release -p devlanes-gui-codec --features h264 --example h264_timing
//! ```

use std::time::{Duration, Instant};

use devlanes_gui_codec::{EncodeParams, Frame, Packet, h264};
use devlanes_gui_host::pattern::draw_grid;
use devlanes_gui_proto::Size;

/// Frames encoded per measurement.
const FRAMES: u32 = 120;

/// The test pattern with a moving bar, so each frame differs from the last.
fn grid(width: u32, height: u32, step: u32) -> Frame {
    let mut pixels = draw_grid(Size::new(width, height));
    let bar = (step * 8) % width;
    for y in 0..height {
        for x in bar..(bar + 16).min(width) {
            let at = ((y * width + x) * 4) as usize;
            pixels[at..at + 4].copy_from_slice(&[0, 0, 0, 0xff]);
        }
    }
    Frame::packed(width, height, pixels).expect("grid")
}

/// Rows of fine coloured strokes on white, scrolled by `step` rows.
fn text(width: u32, height: u32, step: u32) -> Frame {
    let mut pixels = vec![0xffu8; (width * height * 4) as usize];
    for y in 0..height {
        let line = y + step * 2;
        let mut seed = line.wrapping_mul(0x9e37_79b9) | 1;
        for x in 0..width {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            if line % 14 < 11 && x % 8 < 6 && seed.is_multiple_of(3) {
                let at = ((y * width + x) * 4) as usize;
                let colour = match (x / 64 + line / 14) % 3 {
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

/// Milliseconds of `total` per frame.
fn per_frame(total: Duration, frames: u32) -> f64 {
    total.as_secs_f64() * 1000.0 / f64::from(frames)
}

fn main() {
    let found = h264();
    println!(
        "H.264: {:?}, problem {:?}",
        found.version, found.unavailable
    );
    let Some(decoders) = found.decoders.first().cloned() else {
        return;
    };
    for (width, height) in [(1280, 800), (1920, 1080)] {
        for (content, draw) in [
            ("pattern", grid as fn(u32, u32, u32) -> Frame),
            ("text", text),
        ] {
            let frames: Vec<Frame> = (0..FRAMES).map(|i| draw(width, height, i)).collect();
            for encoders in &found.encoders {
                let mut encoder = match encoders.encoder() {
                    Ok(encoder) => encoder,
                    Err(error) => {
                        println!("{:?}: {error}", encoders.offer());
                        continue;
                    }
                };
                let mut decoder = decoders.decoder().expect("a decoder");
                let params = EncodeParams {
                    bitrate_bps: 8_000_000,
                    frame_interval: Duration::from_millis(33),
                    quality: 60,
                    keyframe: false,
                    refine: false,
                };
                let (mut encoding, mut decoding, mut bytes) = (Duration::ZERO, Duration::ZERO, 0);
                let mut failed = None;
                for frame in &frames {
                    let start = Instant::now();
                    let encoded = match encoder.encode(frame, &params) {
                        Ok(Some(encoded)) => encoded,
                        Ok(None) => continue,
                        Err(error) => {
                            failed = Some(error);
                            break;
                        }
                    };
                    encoding += start.elapsed();
                    bytes += encoded.payload.len();
                    let start = Instant::now();
                    decoder
                        .decode(&Packet {
                            payload: &encoded.payload,
                            width,
                            height,
                            keyframe: encoded.keyframe,
                            timestamp: Duration::ZERO,
                        })
                        .expect("decodes");
                    decoding += start.elapsed();
                }
                if let Some(error) = failed {
                    println!(
                        "{width}x{height} {content} {:?}: {error:?}",
                        encoders.offer()
                    );
                    continue;
                }
                println!(
                    "{width}x{height} {content:7} hardware={:5}: encode {:.2} ms, decode {:.2} ms, {} KB/frame",
                    encoders.offer().hardware,
                    per_frame(encoding, FRAMES),
                    per_frame(decoding, FRAMES),
                    bytes / FRAMES as usize / 1024
                );
            }
        }
    }
}
