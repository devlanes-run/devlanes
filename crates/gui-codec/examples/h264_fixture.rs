//! Writes `tests/fixtures/grid-160x90.h264`: three access units of the test
//! pattern's grid at 160x90 (a keyframe and two predicted frames), each
//! preceded by its length as four bytes, big-endian. The macOS decoder's test
//! decodes it, since no Mac encodes in this repository's CI. Uses this
//! machine's software H.264 encoder:
//!
//! ```sh
//! cargo run -p devlanes-gui-codec --features h264 --example h264_fixture
//! ```

use std::time::Duration;

use devlanes_gui_codec::{EncodeParams, Frame, h264};
use devlanes_gui_host::pattern::draw_grid;
use devlanes_gui_proto::Size;

fn main() {
    let found = h264();
    let Some(backend) = found.encoders.iter().find(|e| !e.offer().hardware) else {
        eprintln!("no software H.264 encoder here: {:?}", found.unavailable);
        std::process::exit(1);
    };
    let mut encoder = backend.encoder().expect("an encoder");
    let size = Size::new(160, 90);
    let frame = Frame::packed(size.width, size.height, draw_grid(size)).expect("grid");
    let mut out = Vec::new();
    for _ in 0..3 {
        let params = EncodeParams {
            bitrate_bps: 1_000_000,
            frame_interval: Duration::from_millis(33),
            quality: 60,
            keyframe: false,
            refine: false,
        };
        let encoded = encoder
            .encode(&frame, &params)
            .expect("encodes")
            .expect("a frame");
        out.extend_from_slice(&(encoded.payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&encoded.payload);
    }
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grid-160x90.h264");
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("the fixtures directory");
    std::fs::write(&path, &out).expect("writes");
    println!("wrote {} ({} bytes)", path.display(), out.len());
}
