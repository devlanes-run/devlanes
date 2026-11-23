//! The host's connections to the devlanes daemon, behind traits so the
//! session can be tested without one.

use async_trait::async_trait;
use devlanes_client::{Client, Context, Frame, Method, Paths, RpcError};
use devlanes_proto::{Actor, ActorKind, Control, Event, features};
use serde_json::{Value, json};

/// What the `daemon` event says while no daemon runs.
pub const STOPPED: &str = "devlanes isn't running. Run devlanes in a terminal.";

/// What the `daemon` event says when the daemon is too old for the host.
pub const OUTDATED: &str = "The running devlanes daemon is too old for the browser extension. Restart the daemon with devlanes daemon restart.";

/// Why the host has no usable daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// No daemon answers, with what to tell the person.
    Stopped(String),
    /// The daemon speaks another protocol or lacks a feature the host needs.
    Outdated,
}

/// A failed daemon request.
#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// The daemon answered with an error.
    Rpc(RpcError),
    /// There was no daemon to send the request to.
    Unavailable(Unavailable),
    /// The connection broke, or the reply wasn't what the method returns.
    Lost(String),
}

/// Opens connections to the daemon. Neither ever starts one.
#[async_trait]
pub trait Daemon: Send + Sync + 'static {
    /// A connection for requests.
    async fn queries(&self) -> Result<Box<dyn Queries>, Unavailable>;
    /// A connection subscribed to the daemon's events.
    async fn events(&self) -> Result<Box<dyn Events>, Unavailable>;
}

/// A connection that sends one request at a time.
#[async_trait]
pub trait Queries: Send {
    /// Sends `method` and waits for its answer.
    async fn call(
        &mut self,
        method: Method,
        params: Value,
        context: Context,
    ) -> Result<Value, CallError>;
}

/// A subscription to the daemon's events.
#[async_trait]
pub trait Events: Send {
    /// The next event, or `None` once the subscription ended.
    async fn next(&mut self) -> Option<Event>;
}

/// The context of the host's requests. The cwd is the temporary directory, so
/// the folder Chrome happened to start the host in never picks a project, and
/// requests name their lane instead.
pub fn context(lane: Option<String>) -> Context {
    Context {
        cwd: std::env::temp_dir(),
        actor: Actor {
            kind: ActorKind::Human,
            agent_id: None,
            session_id: None,
        },
        lane,
        ..Context::default()
    }
}

/// The daemon of this user's devlanes home.
pub struct Local;

impl Local {
    /// Connects without spawning, and refuses a daemon without the features
    /// the host relies on.
    async fn connect() -> Result<Client, Unavailable> {
        // Resolving the home reads the filesystem.
        let paths = tokio::task::spawn_blocking(Paths::discover)
            .await
            .map_err(devlanes_client::Error::from)
            .and_then(|paths| paths)
            .map_err(|error| {
                Unavailable::Stopped(format!(
                    "devlanes can't find its home: {error}. Run devlanes doctor."
                ))
            })?;
        match Client::connect(&paths).await {
            // An older daemon would answer `hosts` for the cwd's project only,
            // and count the subscription as a reason to stay up.
            Ok(client)
                if client.supports(features::QUIET_SUBSCRIBE)
                    && client.supports(features::HOSTS_ALL) =>
            {
                Ok(client)
            }
            Ok(_) | Err(devlanes_client::Error::Incompatible { .. }) => Err(Unavailable::Outdated),
            Err(error) if error.is_not_running() => Err(Unavailable::Stopped(STOPPED.into())),
            Err(error) => Err(Unavailable::Stopped(format!(
                "devlanes doesn't answer: {error}. Run devlanes doctor."
            ))),
        }
    }
}

#[async_trait]
impl Daemon for Local {
    /// A plain connection.
    async fn queries(&self) -> Result<Box<dyn Queries>, Unavailable> {
        Ok(Box::new(Self::connect().await?))
    }

    /// A quiet `subscribe`, which neither keeps the daemon past its idle exit
    /// nor keeps its sampler fast, so an open browser costs nothing.
    async fn events(&self) -> Result<Box<dyn Events>, Unavailable> {
        let mut client = Self::connect().await?;
        let id = client
            .begin_request(
                Method::Subscribe,
                json!({"keeps_awake": false}),
                context(None),
            )
            .await
            .map_err(|_| Unavailable::Stopped(STOPPED.into()))?;
        Ok(Box::new(Subscription { client, id }))
    }
}

#[async_trait]
impl Queries for Client {
    /// [`Client::request`], with its errors sorted for the session.
    async fn call(
        &mut self,
        method: Method,
        params: Value,
        context: Context,
    ) -> Result<Value, CallError> {
        self.request(method, params, context)
            .await
            .map_err(|error| match error {
                devlanes_client::Error::Rpc(error) => CallError::Rpc(error),
                error => CallError::Lost(error.to_string()),
            })
    }
}

/// The connection of a `subscribe` request.
struct Subscription {
    client: Client,
    id: u64,
}

#[async_trait]
impl Events for Subscription {
    /// Skips frames other than events. The subscription's own response, or
    /// a broken connection, ends it.
    async fn next(&mut self) -> Option<Event> {
        loop {
            match self.client.next_frame().await {
                Ok(Frame::Control(Control::Event(event))) => return Some(event),
                Ok(Frame::Control(Control::Response(response))) if response.id == self.id => {
                    return None;
                }
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    }
}
