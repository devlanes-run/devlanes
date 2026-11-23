//! The messages of protocol 1 between the extension and the host.

use devlanes_proto::remote::WaitsFor;
use serde::Serialize;
use serde_json::Value;

use crate::view::View;

/// The protocol versions this host speaks.
pub const SUPPORTED: &[u64] = &[1];

/// A request from the extension, read from one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub call: Call,
}

/// What a request asks for. Every method outside this list gets `unknown_method`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Hello { protocol: u64, extension: String },
    Watch,
    Use(UseParams),
    StopAndUse(UseParams),
    Unknown(String),
}

/// `use` and `stop_and_use` params. `host` is `None` for this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseParams {
    pub lane: String,
    pub host: Option<String>,
}

/// A frame that isn't a protocol 1 request. `id` is the request's id when it
/// had a readable one, so the error can answer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub id: Option<u64>,
    pub message: String,
}

impl Request {
    /// Reads one request. Unknown fields are ignored, so a newer extension's
    /// additions don't break an older host.
    pub fn parse(body: &[u8]) -> Result<Self, Invalid> {
        let invalid = |id, message: &str| Invalid {
            id,
            message: format!("{message} Update the devlanes extension and devlanes."),
        };
        let value: Value = serde_json::from_slice(body)
            .map_err(|_| invalid(None, "The extension sent a message that isn't JSON."))?;
        let Some(id) = value.get("id").and_then(Value::as_u64) else {
            return Err(invalid(
                None,
                "The extension sent a request without a numeric id.",
            ));
        };
        let Some(method) = value.get("method").and_then(Value::as_str) else {
            return Err(invalid(
                Some(id),
                "The extension sent a request without a method.",
            ));
        };
        let params = value.get("params").unwrap_or(&Value::Null);
        let call = match method {
            "hello" => {
                let protocol = params.get("protocol").and_then(Value::as_u64);
                let extension = params.get("extension").and_then(Value::as_str);
                let (Some(protocol), Some(extension)) = (protocol, extension) else {
                    return Err(invalid(
                        Some(id),
                        "The extension sent hello without its protocol and version.",
                    ));
                };
                Call::Hello {
                    protocol,
                    extension: extension.to_owned(),
                }
            }
            "watch" => Call::Watch,
            "use" | "stop_and_use" => {
                let lane = params.get("lane").and_then(Value::as_str);
                // `box` must be there, as a name or `null`, so a request that
                // forgets it never moves a lane back to this machine.
                let host = match params.get("box") {
                    Some(Value::Null) => Some(None),
                    Some(Value::String(host)) => Some(Some(host.clone())),
                    _ => None,
                };
                let (Some(lane), Some(host)) = (lane, host) else {
                    return Err(invalid(
                        Some(id),
                        &format!("The extension sent {method} without a lane and a box."),
                    ));
                };
                let params = UseParams {
                    lane: lane.to_owned(),
                    host,
                };
                if method == "use" {
                    Call::Use(params)
                } else {
                    Call::StopAndUse(params)
                }
            }
            other => Call::Unknown(other.to_owned()),
        };
        Ok(Self { id, call })
    }
}

/// Whether the host reaches a daemon it can work with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonState {
    Running,
    Stopped,
    Outdated,
}

/// `hello`'s result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HelloResult {
    pub protocol: u64,
    pub devlanes: String,
    pub daemon: DaemonState,
}

/// `watch`'s result, an empty object. The view follows as events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchResult {}

/// The result of `use` and `stop_and_use`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UseResult {
    pub lane: String,
    #[serde(rename = "box")]
    pub host: Option<String>,
    pub previous: Option<String>,
    pub mirrored: bool,
    pub pending: bool,
    pub waits_for: Option<WaitsFor>,
}

/// An error reply's `error`. `message` is ready to show as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ErrorData>,
}

/// The `data` of the errors whose details the popup acts on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ErrorData {
    /// `protocol_mismatch`.
    Supported { supported: Vec<u64> },
    /// `local_lane_running`.
    LocalLaneRunning {
        holds: String,
        local_lane: String,
        commands: Vec<String>,
    },
    /// `containers_hold_ports`.
    Commands { commands: Vec<String> },
    /// `daemon_failed`: the daemon's own error code.
    Daemon { code: String },
}

impl HostError {
    /// An error without `data`.
    pub fn plain(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            data: None,
        }
    }

    /// `hello_required`: a request came before `hello`.
    pub fn hello_required() -> Self {
        Self::plain(
            "hello_required",
            "The extension sent a request before hello. Reload the devlanes extension.",
        )
    }

    /// `unknown_method`.
    pub fn unknown_method(method: &str) -> Self {
        Self::plain(
            "unknown_method",
            format!("This devlanes doesn't know the request {method}. Update devlanes."),
        )
    }

    /// `protocol_mismatch`, with the versions this host speaks.
    pub fn protocol_mismatch(protocol: u64) -> Self {
        Self {
            code: "protocol_mismatch".into(),
            message: format!(
                "The extension speaks protocol {protocol}, and this devlanes speaks protocol 1. Update devlanes or the extension so they match."
            ),
            data: Some(ErrorData::Supported {
                supported: SUPPORTED.to_vec(),
            }),
        }
    }

    /// `daemon_failed`: the daemon answered `code`, which the host doesn't
    /// map, with `message`.
    pub fn daemon_failed(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: "daemon_failed".into(),
            message: message.into(),
            data: Some(ErrorData::Daemon { code: code.into() }),
        }
    }
}

/// An error reply. `id` is `null` when the request had no readable id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorReply {
    pub id: Option<u64>,
    pub error: HostError,
}

/// A successful reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reply<T> {
    pub id: u64,
    pub result: T,
}

/// A daemon state change, with what to do when the daemon isn't usable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaemonStatus {
    pub state: DaemonState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A `remote.warning` notice the popup shows. `from` and `to` are `None` for
/// this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Notice {
    LaneSwitched {
        lane: String,
        from: Option<String>,
        to: Option<String>,
        message: String,
    },
    ChoiceCleared {
        lane: String,
        local_lane: String,
        holds: String,
        message: String,
    },
}

impl Notice {
    /// The notice for a `remote.warning` event's data, or `None` for a code
    /// the popup doesn't show or data without the fields it needs.
    pub fn of_warning(data: &Value) -> Option<Self> {
        let text = |key: &str| data.get(key).and_then(Value::as_str).map(str::to_owned);
        let lane = text("lane")?;
        let message = text("message")?;
        match data.get("code").and_then(Value::as_str)? {
            "lane_switched" => Some(Self::LaneSwitched {
                lane,
                from: text("from"),
                to: text("to"),
                message,
            }),
            "choice_cleared" => Some(Self::ChoiceCleared {
                lane,
                local_lane: text("local_lane")?,
                holds: text("holds")?,
                message,
            }),
            _ => None,
        }
    }
}

/// An event the host sends after `watch`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    View(View),
    Daemon(DaemonStatus),
    Notice(Notice),
}

/// The shell commands that free a running local lane's ports, from the data
/// of the daemon's `local_lane_running` refusal. The rule is the daemon's
/// `Hold::stop_sentences`: the lane stop while processes hold the ports, then
/// the lane's own Compose project and plain containers.
pub fn stop_commands(data: &Value) -> Vec<String> {
    let mut commands = Vec::new();
    if data.get("holds").and_then(Value::as_str) == Some("processes")
        && let Some(lane) = data.get("local_lane").and_then(Value::as_str)
    {
        commands.push(format!("devlanes stop --lane {lane}"));
    }
    commands.extend(container_commands(data));
    commands
}

/// The `docker` commands that stop the lane's own containers named in the
/// data of `local_lane_running`. Empty when only processes hold the ports.
pub fn container_commands(data: &Value) -> Vec<String> {
    let mut commands = Vec::new();
    if let Some(project) = data.get("compose_project").and_then(Value::as_str) {
        commands.push(format!("docker compose -p {project} stop"));
    }
    let containers: Vec<&str> = data
        .get("containers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !containers.is_empty() {
        commands.push(format!("docker stop {}", containers.join(" ")));
    }
    commands
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// `value` read as a request.
    fn parse(value: Value) -> Result<Request, Invalid> {
        Request::parse(value.to_string().as_bytes())
    }

    #[test]
    fn requests_parse_with_their_params() {
        assert_eq!(
            parse(
                json!({"id": 1, "method": "hello", "params": {"protocol": 1, "extension": "0.1.0"}})
            ),
            Ok(Request {
                id: 1,
                call: Call::Hello {
                    protocol: 1,
                    extension: "0.1.0".into()
                }
            })
        );
        assert_eq!(
            parse(json!({"id": 3, "method": "use", "params": {"lane": "acme@0", "box": null}})),
            Ok(Request {
                id: 3,
                call: Call::Use(UseParams {
                    lane: "acme@0".into(),
                    host: None
                })
            })
        );
        assert_eq!(
            parse(
                json!({"id": 4, "method": "stop_and_use", "params": {"lane": "acme@0", "box": "fork-b"}})
            ),
            Ok(Request {
                id: 4,
                call: Call::StopAndUse(UseParams {
                    lane: "acme@0".into(),
                    host: Some("fork-b".into())
                })
            })
        );
        assert_eq!(
            parse(json!({"id": 5, "method": "lane.clean"})).map(|request| request.call),
            Ok(Call::Unknown("lane.clean".into()))
        );
    }

    #[test]
    fn broken_requests_are_invalid() {
        assert_eq!(Request::parse(b"{").map_err(|e| e.id), Err(None));
        assert_eq!(
            parse(json!({"method": "watch"})).map_err(|e| e.id),
            Err(None)
        );
        assert_eq!(parse(json!({"id": 2})).map_err(|e| e.id), Err(Some(2)));
        // A `use` without `box` must not read as "this machine".
        assert_eq!(
            parse(json!({"id": 3, "method": "use", "params": {"lane": "acme@0"}}))
                .map_err(|e| e.id),
            Err(Some(3))
        );
    }

    #[test]
    fn commands_follow_what_holds_the_ports() {
        assert_eq!(
            stop_commands(&json!({"holds": "processes", "local_lane": "acme@0"})),
            ["devlanes stop --lane acme@0"]
        );
        assert_eq!(
            stop_commands(&json!({
                "holds": "processes",
                "local_lane": "acme@0",
                "compose_project": "acme-lane0",
                "containers": ["acme-cache"],
            })),
            [
                "devlanes stop --lane acme@0",
                "docker compose -p acme-lane0 stop",
                "docker stop acme-cache",
            ]
        );
        assert_eq!(
            stop_commands(&json!({
                "holds": "containers",
                "local_lane": "acme@0",
                "compose_project": null,
                "containers": ["acme-db", "acme-cache"],
            })),
            ["docker stop acme-db acme-cache"]
        );
    }

    #[test]
    fn only_known_notices_are_sent() {
        assert_eq!(
            Notice::of_warning(&json!({
                "host": "fork-b", "code": "lane_switched", "lane": "acme@0",
                "to": "fork-b", "from": null, "message": "acme@0 now comes from fork-b",
            })),
            Some(Notice::LaneSwitched {
                lane: "acme@0".into(),
                from: None,
                to: Some("fork-b".into()),
                message: "acme@0 now comes from fork-b".into(),
            })
        );
        assert_eq!(
            Notice::of_warning(&json!({"code": "lane_skipped", "lane": "acme@0", "message": "x"})),
            None
        );
    }
}
