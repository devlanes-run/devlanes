//! A session against a test-pattern host on loopback TCP: what each step
//! sends reaches the window it picked, with the keys and text as written,
//! and a screenshot comes back as a PNG of the grid.

use std::time::Duration;

use devlanes_gui_host::{
    Echo, ServeOptions, TestPattern, TestPatternOptions, pattern, serve, start_backend,
};
use devlanes_gui_proto::{Buttons, KeyCode, Point, Size, WindowId};
use devlanes_gui_tool::{connect, encode_png, parse_chord};
use tokio::net::TcpListener;

/// Longest the whole test may take.
const PATIENCE: Duration = Duration::from_secs(60);

/// The RGBA pixel at `at` of a decoded PNG of `width` columns.
fn pixel(rgba: &[u8], width: u32, at: Point) -> [u8; 3] {
    let index = (at.y as usize * width as usize + at.x as usize) * 4;
    [rgba[index], rgba[index + 1], rgba[index + 2]]
}

/// Picks the second window by id, sends a click, text, a chord and a resize
/// to it, and reads the echoes; the screenshot after the resize is a PNG of
/// the grid at the new size.
#[tokio::test]
async fn a_session_drives_the_window_it_picked() {
    let (backend, handle) = TestPattern::new(TestPatternOptions::default());
    let second = handle.open_window(Size::new(240, 160));
    let running = start_backend(backend).await.expect("the pattern starts");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let options = ServeOptions::new("acme:desktop");
    let shutdown = options.shutdown.clone();
    let served = tokio::spawn(serve(listener, running, options));

    let work = async {
        let mut session = connect(address, "test").await.expect("connect");
        assert_eq!(session.window(None).expect("main window").id, WindowId(1));
        let window = session
            .window(Some(&second.0.to_string()))
            .expect("second window")
            .id;
        assert_eq!(window, second);

        session
            .click(window, Point::new(30, 40), Buttons::RIGHT, true)
            .await
            .expect("click");
        session.type_text(window, "acme 42").await.expect("type");
        let chord = parse_chord("ctrl+s").expect("chord");
        session.keys(window, &[chord]).await.expect("keys");
        let taken = session
            .resize(window, Size::new(200, 120))
            .await
            .expect("resize");
        assert_eq!(taken, Some(Size::new(200, 120)));

        let frame = session.screenshot(window).await.expect("screenshot");
        let png = encode_png(&frame).expect("png");
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().expect("a PNG header");
        let mut rgba = vec![0; reader.output_buffer_size().expect("buffer size")];
        let info = reader.next_frame(&mut rgba).expect("a PNG picture");
        assert_eq!((info.width, info.height), (200, 120));
        let size = Size::new(200, 120);
        for row in 0..pattern::GRID_ROWS {
            for column in 0..pattern::GRID_COLUMNS {
                let centre = pattern::cell_centre(column, row, size);
                let want = pattern::grid_colour(centre.x as u32, centre.y as u32, size);
                assert_eq!(pixel(&rgba, 200, centre), want, "cell {column},{row}");
            }
        }
        session.close();
    };
    tokio::time::timeout(PATIENCE, work)
        .await
        .expect("the session finishes");

    let ctrl = KeyCode::keyboard(0xe0);
    let s = KeyCode::keyboard(0x16);
    let echoes: Vec<Echo> = handle
        .echoes()
        .into_iter()
        .filter(|echo| !matches!(echo, Echo::Resized { .. }))
        .collect();
    let press = Echo::Pressed {
        window: second,
        position: Point::new(30, 40),
        buttons: Buttons::RIGHT,
    };
    let key = |code, down| Echo::Key {
        window: second,
        code,
        down,
    };
    assert_eq!(
        echoes,
        [
            press.clone(),
            press,
            Echo::Text {
                window: second,
                text: "acme 42".into(),
            },
            key(ctrl, true),
            key(s, true),
            key(s, false),
            key(ctrl, false),
        ]
    );
    shutdown.cancel();
    let _ = served.await;
}
