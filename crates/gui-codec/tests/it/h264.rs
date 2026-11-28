//! H.264 through this machine's backends and back. Each test skips, with the
//! reason printed, where the machine has no H.264 encoder or decoder (a Linux
//! box without OpenH264, a Windows image without Media Foundation's
//! transforms), and fails when a codec takes longer than [`TIMEOUT`].

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use devlanes_gui_codec::{
    DecoderBackend, EncodeParams, Encoded, EncoderBackend, Frame, Packet, VideoDecoder, h264,
};
use devlanes_gui_host::pattern::{draw_grid, shows_grid};
use devlanes_gui_proto::Size;

/// How long one test may take with a real codec.
const TIMEOUT: Duration = Duration::from_secs(120);

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
    let Some(decoder) = found.decoders.first().cloned() else {
        eprintln!("skipped: no H.264 decoder here: {:?}", found.unavailable);
        return Vec::new();
    };
    if found.encoders.is_empty() {
        eprintln!("skipped: no H.264 encoder here: {:?}", found.unavailable);
    }
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

/// The committed stream (`examples/h264_fixture.rs` wrote it from the test
/// pattern with Microsoft's encoder) decodes to the grid with this machine's
/// decoder. It is the macOS decoder's only test, since nothing on a Mac
/// encodes H.264 yet, and it shows every decoder reads another encoder's
/// stream.
#[test]
fn the_committed_stream_decodes_to_the_grid() {
    let Some(decoders) = h264().decoders.first().cloned() else {
        eprintln!("skipped: no H.264 decoder here");
        return;
    };
    bounded(move || {
        let stream = include_bytes!("../fixtures/grid-160x90.h264");
        let mut decoder = decoders.decoder().expect("a decoder");
        let (mut rest, mut pictures) = (&stream[..], 0);
        while let Some((length, after)) = rest.split_first_chunk::<4>() {
            let (unit, next) = after.split_at(u32::from_be_bytes(*length) as usize);
            let packet = Packet {
                payload: unit,
                width: 160,
                height: 90,
                keyframe: pictures == 0,
                timestamp: Duration::ZERO,
            };
            let picture = decoder
                .decode(&packet)
                .expect("decodes")
                .expect("a picture");
            assert!(
                shows_grid(&picture),
                "picture {pictures} from {:?}",
                decoders.offer()
            );
            pictures += 1;
            rest = next;
        }
        assert_eq!(pictures, 3);
    });
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

/// After the picture stops changing, the refinement of the same picture is
/// closer to it than the frames at the pacer's low rate were.
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
            let mut last = None;
            for _ in 0..5 {
                if let Some(encoded) = encoder.encode(&picture, &slow).expect("encodes") {
                    last = Some(decode(&mut *decoder, &encoded));
                }
            }
            let before = distance(&picture, &last.expect("a frame at the low rate"));
            let refine = EncodeParams {
                refine: true,
                quality: 100,
                ..slow
            };
            let refined = encoder
                .encode(&picture, &refine)
                .expect("encodes")
                .expect("a refinement");
            let after = distance(&picture, &decode(&mut *decoder, &refined));
            assert!(
                after < before * 0.8,
                "refined {after:.2} against {before:.2} from {:?}",
                encoders.offer()
            );
        });
    }
}
