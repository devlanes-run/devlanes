//! A session against a test-pattern host on loopback TCP: what each step
//! sends reaches the window it picked, with the keys and text as written,
//! and the steps refuse what the window cannot take.

use std::time::Duration;

use devlanes_gui_host::{
    Echo, ServeOptions, TestPattern, TestPatternOptions, serve, start_backend,
};
use devlanes_gui_proto::{Buttons, KeyCode, MAX_TEXT, Point, Size, WindowId};
use devlanes_gui_tool::{After, ToolError, connect, parse_chord, write_png};
use tokio::net::TcpListener;

/// Longest the whole test may take.
const PATIENCE: Duration = Duration::from_secs(60);

/// Picks the second window by id, sends a click, text longer than one
/// message, a chord and a resize to it, and reads the echoes. A click outside
/// the window is refused, and a screenshot writes a PNG of the window's size.
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
    // Over one message's limit, with a two-byte character across the cut.
    let long: String = format!("{}é tail", "a".repeat(MAX_TEXT - 1));

    let work = async {
        let mut session = connect(address, "test").await.expect("connect");
        assert_eq!(session.window(None).expect("main window").id, WindowId(1));
        let window = session
            .window(Some(&second.0.to_string()))
            .expect("second window")
            .id;
        assert_eq!(window, second);

        let refused = session
            .click(window, Point::new(240, 10), Buttons::LEFT, false)
            .await;
        assert!(
            matches!(refused, Err(ToolError::OutsideWindow { size, .. }) if size == Size::new(240, 160)),
            "{refused:?}"
        );
        let after = session
            .click(window, Point::new(30, 40), Buttons::RIGHT, true)
            .await
            .expect("click");
        assert_eq!(after, After::Open);
        session.type_text(window, &long).await.expect("type");
        let chord = parse_chord("ctrl+s").expect("chord");
        session.keys(window, &[chord]).await.expect("keys");
        // The pattern takes the size; a size it keeps is reported too.
        let taken = session
            .resize(window, Size::new(200, 120))
            .await
            .expect("resize");
        assert_eq!(taken, (Some(Size::new(200, 120)), After::Open));
        let kept = session
            .resize(window, Size::new(200, 120))
            .await
            .expect("resize");
        assert_eq!(kept, (Some(Size::new(200, 120)), After::Open));

        let frame = session.screenshot(window).await.expect("screenshot");
        let mut png = Vec::new();
        write_png(&frame, &mut png).expect("png");
        let info = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .expect("a PNG header")
            .info()
            .clone();
        assert_eq!((info.width, info.height), (200, 120));

        // Another session shrinks the window; a click this session still
        // thinks is inside is dropped by the host and reported as outside.
        let mut other = connect(address, "other").await.expect("connect");
        other
            .resize(window, Size::new(100, 100))
            .await
            .expect("resize");
        other.close();
        let after = session
            .click(window, Point::new(150, 10), Buttons::LEFT, false)
            .await
            .expect("click");
        assert_eq!(after, After::Outside(Size::new(100, 100)));
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
    let (texts, others): (Vec<Echo>, Vec<Echo>) = echoes
        .into_iter()
        .partition(|echo| matches!(echo, Echo::Text { .. }));
    let typed: String = texts
        .iter()
        .map(|echo| match echo {
            Echo::Text { window, text } if *window == second => text.as_str(),
            _ => "",
        })
        .collect();
    assert_eq!(texts.len(), 2, "two pieces");
    assert_eq!(typed, long);
    assert_eq!(
        others,
        [
            press.clone(),
            press,
            key(ctrl, true),
            key(s, true),
            key(s, false),
            key(ctrl, false),
        ]
    );
    shutdown.cancel();
    let _ = served.await;
}
