//! Which frame a local window gets, what its outer close button does, and
//! what a move or resize the app starts turns into locally.
//!
//! The viewer adds nothing of its own: a window whose app draws its own
//! frame and reports when the user drags it is borderless, and a press on the
//! app's title bar or edge becomes a local OS drag. Any other window keeps the
//! viewer OS's frame as the handle to move, minimize and close it.

use devlanes_gui_proto::{Edge, WindowInfo, WindowKind};

/// The viewer's OS, for the few decisions that differ by OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Windows.
    Windows,
    /// macOS.
    Mac,
    /// Linux and other Unix desktops (X11 or Wayland).
    Linux,
}

impl Os {
    /// The OS this build runs on.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// How a local window is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoration {
    /// No frame at all: the app's own title bar is the frame, and the app's
    /// drags become local OS drags. A Windows window keeps its shadow and
    /// rounded corners, which the frame would otherwise carry.
    Borderless,
    /// macOS, where a borderless window cannot be resized from its edges: a
    /// resizable window whose title bar is hidden and transparent, so the
    /// app's frame shows and the OS resizes from the edges.
    HiddenTitleBar,
    /// The viewer OS's own frame around the app's picture.
    Framed,
    /// A menu or tooltip: no frame, no focus, placed at its offset from its
    /// parent where the OS lets a window be placed.
    Popup,
}

/// The frame of `window` on `os`.
pub fn decoration(window: &WindowInfo, os: Os) -> Decoration {
    match window.kind {
        WindowKind::Popup | WindowKind::Tooltip => Decoration::Popup,
        // Without move reports a borderless window could not be moved at all.
        _ if window.draws_own_frame && window.reports_moves => match os {
            Os::Mac => Decoration::HiddenTitleBar,
            Os::Windows | Os::Linux => Decoration::Borderless,
        },
        _ => Decoration::Framed,
    }
}

/// What the outer close button (or Alt+F4, Cmd+W on the frame) of a local
/// window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Close only the local window. The app keeps its window, and the local
    /// one stays closed until `devlanes gui open`.
    Local,
    /// Ask the app to close its window, as its own close button would.
    App,
}

/// The close rule: when the app draws its own frame inside the window, its own
/// close button is there to close the app's window, so the outer one is
/// local; when the viewer's frame is the app's only frame (X11 apps), the
/// outer one is the app's close button.
pub fn close_action(window: &WindowInfo) -> CloseAction {
    if window.draws_own_frame {
        CloseAction::Local
    } else {
        CloseAction::App
    }
}

/// A local drag the viewer starts for a move or resize the app started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drag {
    /// Move the window with the pointer.
    Move,
    /// Resize it from an edge or corner.
    Resize(Edge),
}

/// The local drag for a move (`edge` is `None`) or resize the app started in
/// a window framed as `decoration`, or `None` when the OS frame or the OS
/// edges already do it.
pub fn drag(decoration: Decoration, edge: Option<Edge>) -> Option<Drag> {
    match (decoration, edge) {
        (Decoration::Borderless | Decoration::HiddenTitleBar, None) => Some(Drag::Move),
        (Decoration::Borderless, Some(edge)) => Some(Drag::Resize(edge)),
        // macOS resizes this window from its edges itself and cannot start
        // an edge drag; a framed window's frame is the handle.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devlanes_gui_proto::{LockState, Point, Size, WindowId};

    /// A window described with `draws_own_frame`, `reports_moves` and `kind`.
    fn window(draws_own_frame: bool, reports_moves: bool, kind: WindowKind) -> WindowInfo {
        WindowInfo {
            id: WindowId(1),
            title: "acme".into(),
            app_id: String::new(),
            size: Size::new(640, 480),
            min_size: None,
            max_size: None,
            draws_own_frame,
            reports_moves,
            parent: None,
            offset: Point::new(0, 0),
            lock: LockState::Unlocked,
            kind,
        }
    }

    /// The frame table of the design: an app with its own frame that reports
    /// drags is borderless (a hidden title bar on macOS), and a press on its
    /// title bar or edge becomes a local drag; any other window keeps the OS
    /// frame, which does the moving. The outer close button is local exactly
    /// when the app has a frame of its own inside.
    #[test]
    fn frames_drags_and_close_follow_what_the_host_reports() {
        use Decoration::*;
        let cases = [
            // (own frame, reports moves, OS, frame, move, top-left resize, close)
            (
                true,
                true,
                Os::Linux,
                Borderless,
                Some(Drag::Move),
                Some(Drag::Resize(Edge::TopLeft)),
                CloseAction::Local,
            ),
            (
                true,
                true,
                Os::Windows,
                Borderless,
                Some(Drag::Move),
                Some(Drag::Resize(Edge::TopLeft)),
                CloseAction::Local,
            ),
            (
                true,
                true,
                Os::Mac,
                HiddenTitleBar,
                Some(Drag::Move),
                None,
                CloseAction::Local,
            ),
            // A Windows host in this run: the app's frame inside the OS's.
            (
                true,
                false,
                Os::Linux,
                Framed,
                None,
                None,
                CloseAction::Local,
            ),
            // An X11 app: the viewer's frame is its only one.
            (
                false,
                false,
                Os::Windows,
                Framed,
                None,
                None,
                CloseAction::App,
            ),
            (false, true, Os::Mac, Framed, None, None, CloseAction::App),
        ];
        for (own, moves, os, frame, moved, resized, close) in cases {
            let info = window(own, moves, WindowKind::Toplevel);
            let got = decoration(&info, os);
            let what = format!("own frame {own}, reports moves {moves}, {os:?}");
            assert_eq!(got, frame, "{what}");
            assert_eq!(drag(got, None), moved, "{what}");
            assert_eq!(drag(got, Some(Edge::TopLeft)), resized, "{what}");
            assert_eq!(close_action(&info), close, "{what}");
        }
        let popup = window(true, true, WindowKind::Popup);
        assert_eq!(decoration(&popup, Os::Linux), Popup);
        assert_eq!(drag(Popup, None), None);
    }
}
