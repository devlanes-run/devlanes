//! A GUI client reaching a box's GUI host through the client's mirrored
//! port: the one test of that data path, since two daemons on one machine
//! share loopback otherwise. The box's host is the fake runner's test
//! pattern; the client mirrors its `devlanes_gui` port on `[::1]`.

use std::net::Ipv6Addr;

use devlanes_gui_client::{ClientOptions, DecodedFrame, Event, Events, connect, connect_stream};
use devlanes_gui_host::Command;
use devlanes_gui_proto::{Buttons, Point, Role};
use devlanes_proto::Method;
use serde_json::json;

use super::{BoxSide, ClientSide, SOON, box_lanes, ipv6_loopback};
use crate::{LIMIT, cx, eventually, repo, temp};

/// A box project with one GUI service.
const GUI_BOX: &str = "project: boxgui\nservices:\n  desktop: { run: sleep, gui: true }\n";

/// The next decoded frame on `events`, within [`SOON`].
async fn frame(events: &mut Events) -> DecodedFrame {
    let next = async {
        loop {
            match events.next().await {
                Some(Event::Frame(frame)) => return frame,
                Some(Event::Closed(error)) => panic!("closed before a frame: {error:?}"),
                None => panic!("closed before a frame"),
                Some(_) => {}
            }
        }
    };
    tokio::time::timeout(SOON, next)
        .await
        .expect("a frame in time")
}

/// A viewer's hello, a frame of the test pattern and a click go through the
/// mirror; when the link to the box drops, the viewer's connection ends,
/// and a viewer that dials the held port while the link reconnects is
/// welcomed by the same host once it is back, with frames and input again.
#[tokio::test]
async fn a_viewer_reaches_a_box_gui_host_through_the_mirror_and_across_a_reconnect() {
    if !ipv6_loopback().await {
        eprintln!("skipping: no IPv6 loopback");
        return;
    }
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let main = repo(&tmp.path, "guirepo", GUI_BOX);
        let side = BoxSide::start_gui(&tmp, "box", "17350-17424").await;
        side.h
            .call(Method::Start, json!({}), cx(&main))
            .await
            .expect("box start");
        let port = box_lanes(&side.h)
            .await
            .iter()
            .flat_map(|lane| lane.units.iter())
            .find(|unit| unit.name == "desktop")
            .and_then(|unit| unit.ports.get(devlanes_proto::GUI_PORT).copied())
            .expect("the GUI port");
        let host = side.h.runner.hosts().last().cloned().expect("a GUI host");
        let pattern = host.pattern.clone().expect("a serving host");

        let client = ClientSide::start(&tmp, "client", "17425-17499", &side).await;
        client.connect("box").await;
        let mirrored = eventually(SOON, || async {
            client.mirrored().await.contains(&port).then_some(())
        })
        .await;
        assert!(mirrored.is_some(), "the GUI port is mirrored");

        let viewer = || ClientOptions::new(Role::Viewer, "test-viewer", "test");
        let address = (Ipv6Addr::LOCALHOST, port);
        let (first, mut events) = connect(address, viewer()).await.expect("welcomed");
        let window = first.welcome().windows.first().expect("a window").id;
        assert!(devlanes_gui_host::pattern::shows_grid(&frame(&mut events).await.frame));
        let clicked = |at: Point| {
            let pattern = pattern.clone();
            async move {
                eventually(SOON, || async {
                    pattern
                        .commands()
                        .iter()
                        .any(|command| {
                            matches!(command, Command::Pointer(p) if p.position == at && !p.buttons.is_empty())
                        })
                        .then_some(())
                })
                .await
                .is_some()
            }
        };
        first
            .click(window, Point::new(12, 8), Buttons::LEFT)
            .expect("click");
        assert!(clicked(Point::new(12, 8)).await, "the click reached the box's app");

        // The link drops: the viewer's connection ends with it.
        side.transport.refuse(false);
        side.transport.last_link().expect("link").sever();
        let closed = tokio::time::timeout(SOON, async {
            loop {
                match events.next().await {
                    Some(Event::Closed(_)) | None => return,
                    Some(_) => {}
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "the viewer saw its connection end");

        // A reconnect dials the port the mirror holds while the link is
        // down, and says hello once it is back.
        let down = eventually(SOON, || async {
            (client.host().await["state"] == "reconnecting").then_some(())
        })
        .await;
        assert!(down.is_some(), "the link is down");
        let held = tokio::net::TcpStream::connect(address)
            .await
            .expect("the mirror holds the port");
        side.transport.accept();
        let (second, mut events) = connect_stream(held, viewer())
            .await
            .expect("welcomed by the same host once the link is back");
        assert_eq!(second.welcome().windows.first().map(|w| w.id), Some(window));
        assert!(devlanes_gui_host::pattern::shows_grid(&frame(&mut events).await.frame));
        second
            .click(window, Point::new(30, 20), Buttons::LEFT)
            .expect("click");
        assert!(clicked(Point::new(30, 20)).await, "input reaches it again");

        second.close();
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("in time");
}
