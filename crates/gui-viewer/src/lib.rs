//! The native devlanes GUI viewer: one local window per window of a GUI
//! service, showing its frames and sending local input back.
//!
//! [`run`] takes the main thread for winit's event loop, which macOS and
//! Windows need there (and Windows needs free of multithreaded COM, so
//! nothing else runs on it), and runs the connection to the GUI host and the
//! decoding on a tokio runtime of its own. Frames are presented in software
//! with softbuffer. Every decision sits in [`model::Viewer`], behind the
//! [`model::Windows`] trait; the winit layer only translates between it and
//! winit.
//! The one thing the viewer draws itself is the overlay of a locked window:
//! its last frame dimmed, with one line of text.

mod app;
pub mod frame;
pub mod input;
mod link;
pub mod model;
mod paint;
pub mod resize;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use devlanes_gui_client::instance::Instance;
use devlanes_gui_client::{ClientError, ClientOptions};
use devlanes_gui_codec::DecoderBackend;
use devlanes_gui_proto::Role;
use winit::event_loop::EventLoop;

pub use link::UnitCheck;

/// How often the viewer looks for a `devlanes gui open` asking for its windows.
const SHOW_POLL: Duration = Duration::from_secs(1);

/// What a viewer shows and how it behaves.
pub struct Options {
    /// The GUI host's address: the service's own port, or its mirror here.
    pub address: SocketAddr,
    /// The service, such as `acme@2:desktop`, for the host's logs.
    pub service: String,
    /// The decoders to offer, best first as the codec selector ranks them.
    pub decoders: Vec<Arc<dyn DecoderBackend>>,
    /// Whether windows take focus as they open: yes for `devlanes gui open`,
    /// no when the daemon opens them while the user works elsewhere.
    pub focus: bool,
    /// How to ask the daemon whether the service still runs while the viewer
    /// cannot reconnect; `None` asks nothing, and the viewer then ends only
    /// when the host closes its windows or the process is ended.
    pub check: Option<UnitCheck>,
    /// The lock that makes this the one viewer of the service, which also
    /// carries requests for its windows.
    pub instance: Option<Instance>,
}

/// Why the viewer could not run.
#[derive(Debug, thiserror::Error)]
pub enum ViewerError {
    /// No display to open windows on, such as an SSH session without X11
    /// forwarding or a Linux box without a desktop.
    #[error("there is no display to open windows on: neither WAYLAND_DISPLAY nor DISPLAY is set")]
    NoDisplay,
    /// The OS refused the windowing setup.
    #[error("the windowing system did not start")]
    EventLoop(#[source] winit::error::EventLoopError),
    /// The connection's runtime did not start.
    #[error("the viewer's runtime did not start")]
    Runtime(#[source] std::io::Error),
    /// The first connection to the GUI host failed.
    #[error("cannot connect to the GUI host")]
    Connect(#[source] ClientError),
    /// The connection's task failed with a bug.
    #[error("the viewer's connection failed: {0}")]
    Crashed(String),
}

/// Runs the viewer on this thread, which must be the main thread, until the
/// service is gone or the process is ended.
pub fn run(options: Options) -> Result<(), ViewerError> {
    if !devlanes_gui_client::display::here() {
        return Err(ViewerError::NoDisplay);
    }
    let event_loop = EventLoop::<link::FromLink>::with_user_event()
        .build()
        .map_err(ViewerError::EventLoop)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("devlanes-viewer")
        .enable_all()
        .build()
        .map_err(ViewerError::Runtime)?;
    let proxy = event_loop.create_proxy();
    let tell: link::Tell = Arc::new(move |message| proxy.send_event(message).is_ok());
    let (to_link, from_viewer) = tokio::sync::mpsc::unbounded_channel();
    let client = ClientOptions::new(
        Role::Viewer,
        format!("devlanes-gui-view:{}", std::process::id()),
        user_name(),
    )
    .with_decoders(options.decoders);
    tracing::info!(service = %options.service, address = %options.address, "viewing");
    let frames = link::Frames::default();
    let connection = runtime.spawn(link::run(
        options.address,
        client,
        options.check.clone(),
        from_viewer,
        tell.clone(),
        frames.clone(),
    ));
    // A panic there would leave windows that never change: the viewer ends
    // with an error instead.
    let crashed = tell.clone();
    runtime.spawn(async move {
        if let Err(error) = connection.await
            && error.is_panic()
        {
            let panic = error.into_panic();
            let message = panic
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic".to_owned());
            crashed(link::FromLink::Crashed(message));
        }
    });
    if let Some(mut instance) = options.instance {
        let tell = tell.clone();
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(SHOW_POLL).await;
                let Ok((asked, back)) = tokio::task::spawn_blocking(move || {
                    let asked = instance.show_requested();
                    (asked, instance)
                })
                .await
                else {
                    return;
                };
                instance = back;
                if asked && !tell(link::FromLink::Show) {
                    return;
                }
            }
        });
    }
    let viewer = model::Viewer::new(frame::Os::current(), options.focus, options.check.is_some());
    let mut app = app::App::new(viewer, to_link, frames, tell, runtime.handle().clone());
    let ran = event_loop.run_app(&mut app).map_err(ViewerError::EventLoop);
    runtime.shutdown_timeout(Duration::from_secs(1));
    match app.error {
        Some(error) => Err(error),
        None => ran,
    }
}

// Compiles the README's example with the doc tests, so it keeps matching the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// The local user's name, which other viewers see as the input holder's.
fn user_name() -> String {
    ["USER", "USERNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "viewer".to_owned())
}
