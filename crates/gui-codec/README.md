# devlanes-gui-codec

Video codecs for devlanes GUI streaming: the BGRA frame type, the `VideoEncoder` and `VideoDecoder` traits that each OS backend implements, the `raw` codec every build has, the H.264 backends of each OS, and the selector that picks the codec a window streams in. `devlanes-gui-host` encodes with it and `devlanes-gui-client` decodes with it; the message types come from `devlanes-gui-proto`.

## Usage

```rust
use std::time::Duration;

use devlanes_gui_codec::{
    CodecError, EncodeParams, EncoderBackend, Frame, Packet, RawBackend, RawDecoder,
    VideoDecoder, VideoEncoder, select,
};

fn stream_one_frame(pixels: Vec<u8>, captured: Duration) -> Result<(), CodecError> {
    // The host lists its encoders in its welcome, a viewer its decoders in its
    // hello, and the host picks a codec per viewer.
    let backends = [RawBackend];
    let encoders: Vec<_> = backends.iter().map(EncoderBackend::offer).collect();
    let decoders = [RawDecoder::new().offer()];
    let chosen = select(&encoders, &decoders).expect("both ends have raw");
    assert_eq!(chosen.codec.name(), Some("raw"));

    let mut encoder = backends[0].encoder()?;
    let frame = Frame::packed(64, 32, pixels)?.with_timestamp(captured);
    let params = EncodeParams {
        bitrate_bps: 2_000_000,
        frame_interval: Duration::from_millis(33),
        quality: 60,
        keyframe: true,
        refine: false,
    };
    let encoded = encoder.encode(&frame, &params)?.expect("a keyframe is never skipped");

    let decoded = RawDecoder::new().decode(&Packet {
        payload: &encoded.payload,
        width: encoded.width,
        height: encoded.height,
        keyframe: encoded.keyframe,
        timestamp: captured,
    })?;
    assert_eq!(decoded.map(|f| f.to_packed()), Some(frame.to_packed()));
    Ok(())
}
```

## API

- `Frame`: a BGRA picture with width, height and stride, checked on creation.
- `VideoEncoder`, `VideoDecoder`: one per window and codec; `Send` and synchronous (each call returns the output for its input), so they run on a thread of their own.
- `EncoderBackend`, `DecoderBackend`: what a host or viewer registers per codec implementation, with its `CodecOffer` (codec and whether it is hardware).
- `RawBackend`, `RawEncoder`, `RawDecoder`: full frames compressed with zstd; the encoder skips a picture identical to the previous one.
- `select`: the best codec both ends have, hardware before software and `h264` before `raw`.
- `h264()` (feature `h264`): this machine's H.264 backends and why a part is missing: `OpenH264` on Linux, `MediaFoundationEncoder` (hardware and software) and `MediaFoundationDecoder` on Windows, the `VideoToolbox` decoder on macOS. Only `devlanes-gui` enables the feature; the FFI lives in `devlanes-sys` and `devlanes-sys-windows`.

## H.264

Every H.264 backend converts BGRA to 4:2:0 and back here, with one BT.709 limited-range matrix, about 1.4 ms per 1440x900 frame each way in a release build. H.264 needs even sides: the encoder pads an odd side by repeating the last column or row, the frame message carries the real size, and the decoder crops to it. A new size starts a new encoder, whose first frame is a keyframe. A refinement (`EncodeParams::refine`, asked for by the pacer once a lossy picture stops changing) is coded at a fine quantizer. Decoders decode every frame; the viewer shows the newest.

## License

Apache-2.0
