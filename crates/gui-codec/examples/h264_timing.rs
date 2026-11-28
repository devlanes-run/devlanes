//! Times H.264 on this machine with each encoder found, on the test pattern
//! and on text-like content, at 1920x1080 and 3840x2160: wall time per frame
//! to encode and decode, and the process's CPU time (user plus kernel) per
//! frame with one and with four encoders running at once. Kept for comparing
//! encoders when their order is revisited. Run it in release:
//!
//! ```sh
//! cargo run --release -p devlanes-gui-codec --features h264 --example h264_timing
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use devlanes_gui_codec::{EncodeParams, EncoderBackend, Frame, Packet, h264};
use devlanes_gui_host::pattern::draw_grid;
use devlanes_gui_proto::Size;

/// Frames each encoder encodes per measurement.
const FRAMES: u32 = 60;

/// Distinct pictures cycled through, which keeps 4K memory use bounded.
const PICTURES: u32 = 8;

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

/// This process's CPU time so far, user plus kernel, where the OS reports it.
#[cfg(windows)]
fn cpu_time() -> Option<Duration> {
    use devlanes_sys_windows::{proc, usage};
    let pid = std::process::id();
    let start = proc::start_time(pid).ok()??;
    let usage = usage::process_usage(pid, start).ok()??;
    // In 100 ns units.
    Some(Duration::from_nanos(usage.cpu_time * 100))
}

/// This process's CPU time so far, user plus kernel, where the OS reports it.
#[cfg(target_os = "linux")]
fn cpu_time() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after the command, which may hold spaces, end with `)`;
    // utime and stime are the 14th and 15th, in USER_HZ ticks of 10 ms.
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let ticks: u64 = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    Some(Duration::from_millis(ticks * 10))
}

/// This process's CPU time so far; not measured on this OS.
#[cfg(not(any(windows, target_os = "linux")))]
fn cpu_time() -> Option<Duration> {
    None
}

/// What the pacer asks of a frame on a fast link.
fn params() -> EncodeParams {
    EncodeParams {
        bitrate_bps: 8_000_000,
        frame_interval: Duration::from_millis(33),
        quality: 60,
        keyframe: false,
        refine: false,
    }
}

/// Encodes [`FRAMES`] of `pictures` on each of `streams` encoders at once.
/// Returns the wall time and the process CPU time per frame in
/// milliseconds, and the bytes per frame.
fn concurrent(
    backend: &Arc<dyn EncoderBackend>,
    pictures: &Arc<Vec<Frame>>,
    streams: u32,
) -> Result<(f64, Option<f64>, usize), String> {
    let cpu = cpu_time();
    let start = Instant::now();
    let workers: Vec<_> = (0..streams)
        .map(|_| {
            let (backend, pictures) = (backend.clone(), pictures.clone());
            std::thread::spawn(move || -> Result<usize, String> {
                let mut encoder = backend.encoder().map_err(|e| e.to_string())?;
                let mut bytes = 0;
                for i in 0..FRAMES {
                    let frame = &pictures[(i % PICTURES) as usize];
                    if let Some(encoded) = encoder
                        .encode(frame, &params())
                        .map_err(|e| format!("{e:?}"))?
                    {
                        bytes += encoded.payload.len();
                    }
                }
                Ok(bytes)
            })
        })
        .collect();
    let mut bytes = 0;
    for worker in workers {
        bytes += worker
            .join()
            .map_err(|_| "an encoder panicked".to_owned())??;
    }
    let frames = f64::from(FRAMES * streams);
    let wall = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);
    let cpu = cpu
        .zip(cpu_time())
        .map(|(before, after)| (after - before).as_secs_f64() * 1000.0 / frames);
    Ok((wall, cpu, bytes / (FRAMES * streams) as usize))
}

/// Milliseconds per frame to decode the output of `backend` on `pictures`.
fn decoding(
    backend: &Arc<dyn EncoderBackend>,
    decoders: &Arc<dyn devlanes_gui_codec::DecoderBackend>,
    pictures: &[Frame],
) -> Result<f64, String> {
    let mut encoder = backend.encoder().map_err(|e| e.to_string())?;
    let mut decoder = decoders.decoder().map_err(|e| e.to_string())?;
    let mut spent = Duration::ZERO;
    for i in 0..FRAMES {
        let frame = &pictures[(i % PICTURES) as usize];
        let Some(encoded) = encoder
            .encode(frame, &params())
            .map_err(|e| format!("{e:?}"))?
        else {
            continue;
        };
        let start = Instant::now();
        decoder
            .decode(&Packet {
                payload: &encoded.payload,
                width: frame.width(),
                height: frame.height(),
                keyframe: encoded.keyframe,
                timestamp: Duration::ZERO,
            })
            .map_err(|e| format!("{e:?}"))?;
        spent += start.elapsed();
    }
    Ok(spent.as_secs_f64() * 1000.0 / f64::from(FRAMES))
}

fn main() {
    let found = h264();
    println!(
        "H.264: {:?}, encode problem {:?}, decode problem {:?}",
        found.version, found.encode, found.decode
    );
    let ms = |value: Option<f64>| value.map_or_else(|| "n/a".to_owned(), |v| format!("{v:.2} ms"));
    for (width, height) in [(1920, 1080), (3840, 2160)] {
        for (content, draw) in [
            ("pattern", grid as fn(u32, u32, u32) -> Frame),
            ("text", text),
        ] {
            let pictures: Arc<Vec<Frame>> =
                Arc::new((0..PICTURES).map(|i| draw(width, height, i)).collect());
            for backend in &found.encoders {
                let kind = if backend.offer().hardware {
                    "hardware"
                } else {
                    "software"
                };
                if !backend.fits(width, height) {
                    println!("{width}x{height} {content:7} {kind}: too large for this encoder");
                    continue;
                }
                let decode = match found.decoders.first() {
                    Some(decoders) => decoding(backend, decoders, &pictures).map(Some),
                    None => Ok(None),
                };
                for streams in [1, 4] {
                    match concurrent(backend, &pictures, streams) {
                        Ok((wall, cpu, bytes)) => println!(
                            "{width}x{height} {content:7} {kind} x{streams}: wall {wall:.2} ms/frame, CPU {} per frame, {} KB/frame{}",
                            ms(cpu),
                            bytes / 1024,
                            match (&decode, streams) {
                                (Ok(Some(decode)), 1) => format!(", decode {decode:.2} ms"),
                                (Err(error), 1) => format!(", decode failed: {error}"),
                                _ => String::new(),
                            }
                        ),
                        Err(error) => {
                            println!("{width}x{height} {content:7} {kind} x{streams}: {error}")
                        }
                    }
                }
            }
        }
    }
}
