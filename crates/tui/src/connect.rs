//! The boxes view's modals: connecting to a box, with each question its link
//! asks on the way (an ssh passphrase, a host key, a pairing code), and the
//! pairing code another machine needs to connect to this one. They hold what
//! the modal shows and turn keys into [`Action`]s; the app runs the effects.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

use crate::text;
use crate::words::Worded;

/// How a new box is reached.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Reach {
    /// The name is an SSH destination, as `ssh <name>` takes it.
    #[default]
    Ssh,
    /// A command runs `devlanes tunnel --stdio` in the box, saved as
    /// `remote.boxes.<name>.command`.
    Command,
}

/// The row of the form that takes keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Field {
    /// SSH name or command.
    Reach,
    #[default]
    Name,
    Command,
}

/// The new connection form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form {
    pub reach: Reach,
    pub field: Field,
    pub name: String,
    /// The command as typed, split like a shell line when sent.
    pub command: String,
    /// What is wrong with what was sent.
    pub error: Option<String>,
}

impl Form {
    /// The rows Tab moves through, top to bottom.
    fn fields(&self) -> Vec<Field> {
        match self.reach {
            Reach::Ssh => vec![Field::Reach, Field::Name],
            Reach::Command => vec![Field::Reach, Field::Name, Field::Command],
        }
    }

    /// The text field with the focus, if a text field has it.
    fn text(&mut self) -> Option<&mut String> {
        match self.field {
            Field::Reach => None,
            Field::Name => Some(&mut self.name),
            Field::Command => Some(&mut self.command),
        }
    }

    /// What Enter sends: the box to connect, with its command for a box
    /// reached by one, or what is wrong in words.
    pub fn submit(&self) -> Result<(String, Option<Vec<String>>), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Type the box's name.".into());
        }
        if name.starts_with('-') || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("A box name cannot start with - or hold spaces.".into());
        }
        if self.reach == Reach::Ssh {
            return Ok((name.to_owned(), None));
        }
        // The name becomes a key of remote.boxes in config.yaml.
        if !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(
                "A box reached by a command needs a name of letters, digits, - and _.".into(),
            );
        }
        let command = split_command(&self.command)?;
        if command.is_empty() {
            return Err("Type the command that runs devlanes tunnel --stdio in the box.".into());
        }
        Ok((name.to_owned(), Some(command)))
    }
}

/// A question the box's link asks while the modal is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// Where the answer goes, as the daemon's `remote.prompt` names it.
    pub stream_id: u64,
    /// `code` for a pairing code, `secret` for a passphrase or password
    /// (shown masked), anything else for text such as a host key's yes.
    pub kind: String,
    /// What ssh or the daemon asked, cleaned.
    pub text: String,
    pub input: String,
}

impl Prompt {
    /// Whether the answer is shown as dots.
    pub fn masked(&self) -> bool {
        self.kind == "secret"
    }
}

/// The link's last state, as `remote.state` events describe it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Link {
    pub state: String,
    pub attempt: Option<u64>,
    pub error: Option<String>,
    pub retry_in_ms: Option<u64>,
    /// Seconds since the state arrived, so a retry counts down.
    pub age: u64,
    /// Notices ssh printed on the way, such as a host key it added.
    pub notes: Vec<String>,
}

impl Link {
    /// Takes a `remote.state` event's data.
    pub fn update(&mut self, data: &Value) {
        let text = |key: &str| data.get(key).and_then(Value::as_str).map(text::clean);
        self.state = text("state").unwrap_or_default();
        self.attempt = data.get("attempt").and_then(Value::as_u64);
        self.error = text("error").filter(|error| !error.is_empty());
        self.retry_in_ms = data.get("retry_in_ms").and_then(Value::as_u64);
        self.age = 0;
    }

    /// The state in words: `connecting…`, `trying again in 4s`.
    pub fn words(&self) -> String {
        match self.state.as_str() {
            "" | "connecting" => match self.attempt {
                Some(attempt) if attempt > 1 => format!("connecting, try {attempt}…"),
                _ => "connecting…".into(),
            },
            "reconnecting" => {
                let left = self
                    .retry_in_ms
                    .map(|ms| ms.saturating_sub(self.age * 1000).div_ceil(1000));
                match left {
                    Some(seconds) if seconds > 0 => format!("trying again in {seconds}s"),
                    _ => "trying again…".into(),
                }
            }
            "needs_auth" => "waiting for a sign-in…".into(),
            "needs_pairing" => "waiting for a pairing code…".into(),
            "connected" => "connected".into(),
            state => text::clean(&state.replace('_', " ")),
        }
    }
}

/// One step of the connect modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The new connection form ([`ConnectDialog::form`]).
    Form,
    /// The box's command is being written to config.yaml.
    Saving,
    /// `connect` is attached and the link has not settled.
    Waiting,
    /// The link asks something; the answer goes back on the connection.
    Prompt(Prompt),
    /// Connecting ended without a connection.
    Failed(Worded),
}

/// The connect modal: the form, then the attached `connect` with its states,
/// questions and outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectDialog {
    /// The box, once sent; empty on the form.
    pub host: String,
    /// The `connect` this modal follows; messages of an earlier one are stale.
    pub generation: u64,
    pub form: Form,
    pub link: Link,
    pub step: Step,
}

/// What a key in the connect modal asks the app for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Stay,
    /// Close; a `connect` it follows stays in the daemon.
    Close,
    /// Connect to [`ConnectDialog::host`].
    Connect,
    /// Save the command for [`ConnectDialog::host`], then connect.
    Save(Vec<String>),
    /// Send `answer` for the prompt on `stream_id`.
    Answer {
        stream_id: u64,
        answer: String,
    },
}

impl ConnectDialog {
    /// The empty form.
    pub fn new() -> Self {
        Self {
            host: String::new(),
            generation: 0,
            form: Form::default(),
            link: Link::default(),
            step: Step::Form,
        }
    }

    /// Following a `connect` to `host` from the start, for a box the view
    /// lists already.
    pub fn following(host: &str) -> Self {
        Self {
            host: host.to_owned(),
            step: Step::Waiting,
            ..Self::new()
        }
    }

    /// Whether a `connect` is attached, so closing leaves it to the daemon.
    pub fn attached(&self) -> bool {
        matches!(self.step, Step::Waiting | Step::Prompt(_))
    }

    /// Whether a spinner turns in the modal.
    pub fn busy(&self) -> bool {
        matches!(self.step, Step::Saving | Step::Waiting)
    }

    /// Takes a key.
    pub fn key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc {
            return Action::Close;
        }
        match &mut self.step {
            Step::Form => self.form_key(key, ctrl),
            Step::Prompt(prompt) => match key.code {
                KeyCode::Enter => Action::Answer {
                    stream_id: prompt.stream_id,
                    // An empty frame cancels, so an empty answer is a newline.
                    answer: format!("{}\n", prompt.input),
                },
                KeyCode::Backspace => {
                    prompt.input.pop();
                    Action::Stay
                }
                KeyCode::Char('u') if ctrl => {
                    prompt.input.clear();
                    Action::Stay
                }
                KeyCode::Char(c) if !ctrl => {
                    prompt.input.push(c);
                    Action::Stay
                }
                _ => Action::Stay,
            },
            Step::Failed(_) => match key.code {
                KeyCode::Enter => Action::Connect,
                KeyCode::Char('q') => Action::Close,
                _ => Action::Stay,
            },
            Step::Saving | Step::Waiting => Action::Stay,
        }
    }

    /// Keys on the form: Tab moves between the rows, left, right and space
    /// switch how the box is reached, Enter sends it.
    fn form_key(&mut self, key: KeyEvent, ctrl: bool) -> Action {
        let form = &mut self.form;
        let fields = form.fields();
        let at = fields.iter().position(|f| *f == form.field).unwrap_or(1);
        match key.code {
            KeyCode::Tab | KeyCode::Down => form.field = fields[(at + 1) % fields.len()],
            KeyCode::BackTab | KeyCode::Up => {
                form.field = fields[(at + fields.len() - 1) % fields.len()];
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if form.field == Field::Reach => {
                form.reach = match form.reach {
                    Reach::Ssh => Reach::Command,
                    Reach::Command => Reach::Ssh,
                };
                form.error = None;
            }
            KeyCode::Enter => match form.submit() {
                Ok((host, command)) => {
                    form.error = None;
                    self.host = host;
                    return match command {
                        Some(command) => Action::Save(command),
                        None => Action::Connect,
                    };
                }
                Err(error) => form.error = Some(error),
            },
            KeyCode::Backspace => {
                if let Some(text) = form.text() {
                    text.pop();
                }
            }
            KeyCode::Char('u') if ctrl => {
                if let Some(text) = form.text() {
                    text.clear();
                }
            }
            KeyCode::Char(c) if !ctrl => {
                if let Some(text) = form.text() {
                    text.push(c);
                }
            }
            _ => {}
        }
        Action::Stay
    }

    /// Pasted text, into the field or answer that takes typing; line breaks
    /// are left out so a paste never sends.
    pub fn paste(&mut self, pasted: &str) {
        let pasted: String = pasted.chars().filter(|c| !c.is_control()).collect();
        match &mut self.step {
            Step::Form => {
                if let Some(text) = self.form.text() {
                    text.push_str(&pasted);
                }
            }
            Step::Prompt(prompt) => prompt.input.push_str(&pasted),
            _ => {}
        }
    }

    /// Takes a `remote.prompt` event's data: a question to answer, or `None`
    /// for a notice, which is kept in [`Link::notes`] and needs an empty
    /// answer on the returned stream.
    pub fn prompt(&mut self, data: &Value) -> Option<u64> {
        let stream_id = data.get("stream_id").and_then(Value::as_u64)?;
        let field = |key: &str| {
            data.get(key)
                .and_then(Value::as_str)
                .map(text::clean)
                .unwrap_or_default()
        };
        let kind = field("kind");
        let asked = field("prompt");
        if kind == "notify" {
            self.link.notes.push(asked);
            return Some(stream_id);
        }
        self.step = Step::Prompt(Prompt {
            stream_id,
            kind,
            text: asked,
            input: String::new(),
        });
        None
    }
}

impl Default for ConnectDialog {
    fn default() -> Self {
        Self::new()
    }
}

/// `line` split into a program and its arguments, the way a shell splits
/// words: spaces separate them, single quotes keep everything, double quotes
/// keep spaces, and a backslash keeps the quote, space or backslash after
/// it. A backslash before anything else is kept, so Windows paths
/// (`C:\tools\x.exe`) type as they are.
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    // A word started, even an empty `''`.
    let mut started = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err(unclosed()),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') if matches!(chars.peek(), Some('"' | '\\')) => {
                            word.extend(chars.next());
                        }
                        Some(c) => word.push(c),
                        None => return Err(unclosed()),
                    }
                }
            }
            '\\' if chars
                .peek()
                .is_some_and(|next| matches!(next, '"' | '\'' | '\\') || next.is_whitespace()) =>
            {
                started = true;
                word.extend(chars.next());
            }
            c => {
                started = true;
                word.push(c);
            }
        }
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

/// The error of a quote without its end.
fn unclosed() -> String {
    "A quote in the command is not closed.".into()
}

/// The pairing code modal's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairState {
    /// `remote.pair` is out.
    Loading,
    /// A code, and how long it was valid for when it arrived.
    Code {
        code: String,
        valid_ms: Option<u64>,
    },
    /// Wrong codes paused pairing for `wait_ms` from when it arrived.
    Paused {
        wait_ms: u64,
    },
    /// This machine turns clients away. `invalid` says the remote settings
    /// are invalid rather than `remote.allow_connections` being false.
    Off {
        invalid: bool,
    },
    Failed(Worded),
}

/// The pairing code modal (`p` in the boxes view).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairDialog {
    pub state: PairState,
    /// Seconds since the state arrived, for the countdowns.
    pub age: u64,
}

impl PairDialog {
    /// Waiting for a code.
    pub fn loading() -> Self {
        Self {
            state: PairState::Loading,
            age: 0,
        }
    }

    /// Takes `remote.pair`'s answer: `pairing_paused` and `connections_off`
    /// are states of their own, other errors are shown in words.
    pub fn answered(&mut self, result: Result<Value, crate::ApiError>) {
        self.age = 0;
        self.state = match result {
            Ok(value) => match value.get("code").and_then(Value::as_str) {
                Some(code) => PairState::Code {
                    code: text::clean(code),
                    valid_ms: value.get("valid_ms").and_then(Value::as_u64),
                },
                None => PairState::Failed(Worded::local("the daemon sent no code")),
            },
            Err(crate::ApiError::Rpc(error)) if error.code == "pairing_paused" => {
                PairState::Paused {
                    wait_ms: error
                        .data
                        .get("retry_after_ms")
                        .and_then(Value::as_u64)
                        .unwrap_or(10 * 60_000),
                }
            }
            Err(crate::ApiError::Rpc(error)) if error.code == "connections_off" => PairState::Off {
                invalid: error.data.get("invalid").and_then(Value::as_bool) == Some(true),
            },
            Err(error) => PairState::Failed(crate::words::api(&error)),
        };
    }

    /// Milliseconds left of `total` after [`PairDialog::age`] seconds.
    pub fn left(&self, total: u64) -> u64 {
        total.saturating_sub(self.age * 1000)
    }

    /// The command the other machine runs with the code shown, if one is.
    pub fn command(&self) -> Option<String> {
        match &self.state {
            PairState::Code { code, valid_ms }
                if valid_ms.is_none_or(|valid| self.left(valid) > 0) =>
            {
                Some(format!("devlanes remote connect <box> --code {code}"))
            }
            _ => None,
        }
    }
}

/// `9m 58s`, `45s`: a countdown of `ms`, rounded up to the second.
pub fn countdown(ms: u64) -> String {
    let seconds = ms.div_ceil(1000);
    match (seconds / 60, seconds % 60) {
        (0, s) => format!("{s}s"),
        (m, 0) => format!("{m}m"),
        (m, s) => format!("{m}m {s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Words split on spaces; quotes group them and backslashes keep what
    /// they quote, while other backslashes stay for Windows paths.
    #[test]
    fn commands_split_like_a_shell_line() {
        let split = |line: &str| split_command(line).expect("split");
        assert_eq!(
            split("docker exec -i acme devlanes tunnel --stdio"),
            [
                "docker", "exec", "-i", "acme", "devlanes", "tunnel", "--stdio"
            ]
        );
        assert_eq!(
            split(r#"  wsl.exe -d "Ubuntu 24.04"  devlanes 'tunnel' --stdio "#),
            [
                "wsl.exe",
                "-d",
                "Ubuntu 24.04",
                "devlanes",
                "tunnel",
                "--stdio"
            ]
        );
        assert_eq!(
            split(r#"C:\tools\box.exe a\ b "say \"hi\"" ''"#),
            [r"C:\tools\box.exe", "a b", r#"say "hi""#, ""]
        );
        assert_eq!(split("   "), Vec::<String>::new());
        assert!(split_command("docker 'exec").is_err());
        assert!(split_command("docker \"exec").is_err());
    }

    /// The form says what is wrong in words; a box reached by a command
    /// needs a name config.yaml can hold.
    #[test]
    fn the_form_checks_what_was_typed() {
        let form = |reach, name: &str, command: &str| Form {
            reach,
            name: name.into(),
            command: command.into(),
            ..Form::default()
        };
        assert_eq!(
            form(Reach::Ssh, " me@203.0.113.4 ", "").submit(),
            Ok(("me@203.0.113.4".into(), None))
        );
        assert!(form(Reach::Ssh, "", "").submit().is_err());
        assert!(form(Reach::Ssh, "-oProxyCommand=x", "").submit().is_err());
        assert!(form(Reach::Ssh, "two words", "").submit().is_err());
        assert_eq!(
            form(
                Reach::Command,
                "acme",
                "docker exec -i acme devlanes tunnel --stdio"
            )
            .submit(),
            Ok((
                "acme".into(),
                Some(
                    [
                        "docker", "exec", "-i", "acme", "devlanes", "tunnel", "--stdio"
                    ]
                    .map(String::from)
                    .to_vec()
                )
            ))
        );
        assert!(form(Reach::Command, "me@box", "x").submit().is_err());
        assert!(form(Reach::Command, "acme", "  ").submit().is_err());
    }

    /// Tab goes through the rows, space switches how the box is reached,
    /// letters go to the field with the focus and Enter sends the form.
    #[test]
    fn form_keys() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let mut dialog = ConnectDialog::new();
        for c in "acme".chars() {
            dialog.key(key(KeyCode::Char(c)));
        }
        dialog.key(key(KeyCode::BackTab));
        assert_eq!(dialog.form.field, Field::Reach);
        dialog.key(key(KeyCode::Char(' ')));
        assert_eq!(dialog.form.reach, Reach::Command);
        dialog.key(key(KeyCode::Tab));
        dialog.key(key(KeyCode::Tab));
        assert_eq!(dialog.form.field, Field::Command);
        dialog.paste("wsl.exe -d Ubuntu\n");
        assert_eq!(dialog.form.command, "wsl.exe -d Ubuntu");
        let sent = dialog.key(key(KeyCode::Enter));
        assert_eq!(
            sent,
            Action::Save(["wsl.exe", "-d", "Ubuntu"].map(String::from).to_vec())
        );
        assert_eq!(dialog.host, "acme");
        assert_eq!(dialog.key(key(KeyCode::Esc)), Action::Close);
    }

    /// A question becomes the prompt step and its answer ends in a newline;
    /// a notice is kept and answered at once.
    #[test]
    fn prompts_and_notices() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let mut dialog = ConnectDialog::following("hetzner");
        let notice =
            json!({"kind": "notify", "prompt": "Warning: added the host key", "stream_id": 3});
        assert_eq!(dialog.prompt(&notice), Some(3));
        assert_eq!(dialog.link.notes, ["Warning: added the host key"]);
        assert_eq!(dialog.step, Step::Waiting);
        let secret = json!({"kind": "secret", "prompt": "Enter passphrase:", "stream_id": 4});
        assert_eq!(dialog.prompt(&secret), None);
        dialog.key(key(KeyCode::Char('h')));
        dialog.key(key(KeyCode::Char('i')));
        assert!(matches!(&dialog.step, Step::Prompt(p) if p.masked() && p.input == "hi"));
        assert_eq!(
            dialog.key(key(KeyCode::Enter)),
            Action::Answer {
                stream_id: 4,
                answer: "hi\n".into()
            }
        );
    }

    /// The pairing modal counts its code down from when it arrived and has
    /// states of its own for a pause and for connections being off.
    #[test]
    fn pairing_states() {
        let mut dialog = PairDialog::loading();
        dialog.answered(Ok(json!({"code": "K7QM-3XWD", "valid_ms": 600_000})));
        dialog.age = 2;
        assert!(matches!(&dialog.state, PairState::Code { code, .. } if code == "K7QM-3XWD"));
        assert_eq!(countdown(dialog.left(600_000)), "9m 58s");
        assert_eq!(
            dialog.command().as_deref(),
            Some("devlanes remote connect <box> --code K7QM-3XWD")
        );
        dialog.age = 600;
        assert_eq!(dialog.command(), None, "an expired code is not offered");
        let refused = |code: &str, data| {
            let mut dialog = PairDialog::loading();
            dialog.answered(Err(crate::ApiError::Rpc(devlanes_proto::RpcError::new(
                code, data, 4,
            ))));
            dialog.state
        };
        assert_eq!(
            refused("pairing_paused", json!({"retry_after_ms": 252_000})),
            PairState::Paused { wait_ms: 252_000 }
        );
        assert_eq!(
            refused("connections_off", json!({"invalid": false})),
            PairState::Off { invalid: false }
        );
        assert_eq!(countdown(45_000), "45s");
        assert_eq!(countdown(120_000), "2m");
    }
}
