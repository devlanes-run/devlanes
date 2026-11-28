//! The boxes view's modals: connecting to a box by a pairing ticket, its SSH
//! name or a command, with each question its link asks on the way (an ssh
//! passphrase, a host key, a pairing code), and the ticket and code another
//! machine needs to connect to this one. They hold what the modal shows and
//! turn keys into [`Action`]s; the app runs the effects.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::keys::{KeyCode, KeyEvent, KeyModifiers};
use crate::text;
use crate::words::Worded;

/// How a new box is reached.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Reach {
    /// A pairing ticket from `devlanes daemon pair` on the box: this
    /// machine pairs with the box and reaches it peer to peer.
    #[default]
    Ticket,
    /// The name is an SSH destination, as `ssh <name>` takes it.
    Ssh,
    /// A command runs `devlanes tunnel --stdio` in the box, saved as
    /// `remote.boxes.<name>.command`.
    Command,
}

impl Reach {
    /// The next way in the reach row, `-1` for the one before, wrapping.
    fn step(self, delta: i8) -> Self {
        const ALL: [Reach; 3] = [Reach::Ticket, Reach::Ssh, Reach::Command];
        let at = ALL.iter().position(|reach| *reach == self).unwrap_or(0);
        let next = if delta < 0 {
            at + ALL.len() - 1
        } else {
            at + 1
        };
        ALL[next % ALL.len()]
    }
}

/// The row of the form that takes keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Field {
    /// Ticket, SSH name or command.
    Reach,
    /// Where the ticket is pasted, the first row a new form types into.
    #[default]
    Ticket,
    Name,
    Command,
}

/// A pairing ticket as pasted or sent. A ticket is a secret until it is
/// used or expires, so debug output shows its length only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TicketText(String);

impl TicketText {
    /// `text`, as sent to the daemon.
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The ticket itself, for the request that sends it and the clipboard.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for TicketText {
    /// The length only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TicketText({} chars)", self.0.chars().count())
    }
}

/// The ticket field as the form shows it: the prefix and the length, never
/// the rest, so the secret is not left on screen. `dlp1… 177 characters`,
/// counted as it will be sent.
pub fn ticket_shown(typed: &str) -> String {
    let typed = pasted_ticket(typed);
    let count = typed.chars().count();
    if count <= devlanes_proto::remote::TICKET_PREFIX.len() {
        return text::clean(&typed);
    }
    let start: String = typed
        .chars()
        .take(devlanes_proto::remote::TICKET_PREFIX.len())
        .collect();
    format!("{}… {count} characters", text::clean(&start))
}

/// A ticket as pasted: no whitespace anywhere, since a terminal that wraps
/// a long line may add line breaks or spaces inside it and no ticket holds
/// any, and without the quotes or backticks a copy from Markdown brings at
/// its ends, as the CLI trims them.
fn pasted_ticket(typed: &str) -> String {
    let joined: String = typed.chars().filter(|c| !c.is_whitespace()).collect();
    devlanes_cli::pasted(&joined).to_owned()
}

/// The new connection form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form {
    pub reach: Reach,
    pub field: Field,
    /// The pasted ticket, sent once and then dropped from the form.
    pub ticket: TicketText,
    pub name: String,
    /// The command as typed, split like a shell line when sent.
    pub command: String,
    /// What is wrong with what was sent, or what Enter again would do.
    pub error: Option<String>,
    /// `remote.boxes` of the user config: each box's command by name.
    pub known: BTreeMap<String, Vec<String>>,
    /// Boxes with a link in the daemon now, which keeps its command until it
    /// is disconnected.
    pub live: Vec<String>,
    /// The box whose configured command Enter replaces, once asked.
    replace: Option<String>,
}

/// What the form asks for once it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Submit {
    /// Connect to the box by its name, with the command the config has for
    /// it if any.
    Connect(String),
    /// Save the box's new command to the user config, then connect.
    Save(String, Vec<String>),
    /// Pair with the box in the ticket, under `name` or the box's own name.
    Pair(TicketText, Option<String>),
}

impl Form {
    /// The rows Tab moves through, top to bottom.
    fn fields(&self) -> Vec<Field> {
        match self.reach {
            Reach::Ticket => vec![Field::Reach, Field::Ticket, Field::Name],
            Reach::Ssh => vec![Field::Reach, Field::Name],
            Reach::Command => vec![Field::Reach, Field::Name, Field::Command],
        }
    }

    /// The text field with the focus, if a text field has it. Typing there
    /// takes back a question about replacing a command.
    fn text(&mut self) -> Option<&mut String> {
        self.replace = None;
        match self.field {
            Field::Reach => None,
            Field::Ticket => Some(&mut self.ticket.0),
            Field::Name => Some(&mut self.name),
            Field::Command => Some(&mut self.command),
        }
    }

    /// The configured command of the SSH name typed, which `connect` uses
    /// instead of ssh.
    pub fn configured(&self) -> Option<&Vec<String>> {
        match self.reach {
            Reach::Ssh => self.known.get(self.name.trim()),
            Reach::Ticket | Reach::Command => None,
        }
    }

    /// What Enter sends for a ticket, or what is wrong in words. Nothing
    /// here repeats what was pasted, which may be most of a ticket. The name
    /// follows the daemon's rule for names of boxes paired by ticket; the
    /// daemon words what it refuses beyond that.
    fn submit_ticket(&mut self) -> Result<Submit, String> {
        let ticket = pasted_ticket(&self.ticket.0);
        if ticket.is_empty() {
            return Err("Paste the ticket from devlanes daemon pair on the box.".into());
        }
        if devlanes_cli::cut_ticket(&ticket) {
            return Err(
                "This looks like a pairing ticket cut short. Copy the whole ticket again from \
                 devlanes daemon pair on the box."
                    .into(),
            );
        }
        if !devlanes_cli::is_ticket(&ticket) {
            return Err(
                "This is not a pairing ticket. A ticket starts with dlp1; to reach the box over \
                 SSH or a command, choose that at Reach by."
                    .into(),
            );
        }
        let name = self.name.trim();
        if !name.is_empty()
            && (name.len() > 63
                || name.starts_with('-')
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        {
            return Err(
                "A box name is 1 to 63 letters, digits, - and _, and cannot start with -. Leave \
                 it empty for the box's own name."
                    .into(),
            );
        }
        let name = (!name.is_empty()).then(|| name.to_owned());
        self.ticket = TicketText::default();
        Ok(Submit::Pair(TicketText(ticket), name))
    }

    /// What Enter sends, or what is wrong in words. A command that would
    /// replace the configured one is asked about first, and a box with a link
    /// keeps its command until it is disconnected, so it is not saved then.
    pub(crate) fn submit(&mut self) -> Result<Submit, String> {
        if self.reach == Reach::Ticket {
            return self.submit_ticket();
        }
        let name = self.name.trim().to_owned();
        if name.is_empty() {
            return Err("Type the box's name.".into());
        }
        if name.starts_with('-') || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("A box name cannot start with - or hold spaces.".into());
        }
        if self.reach == Reach::Ssh {
            return Ok(Submit::Connect(name));
        }
        // The name becomes a key of remote.boxes in config.yaml.
        if name.len() > 63
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(
                "A box reached by a command needs a name of 1 to 63 letters, digits, - and _."
                    .into(),
            );
        }
        let command = split_command(&self.command)?;
        if command.is_empty() {
            return Err("Type the command that runs devlanes tunnel --stdio in the box.".into());
        }
        let known = self.known.get(&name);
        if known == Some(&command) {
            return Ok(Submit::Connect(name));
        }
        if self.live.contains(&name) {
            return Err(format!(
                "{name} is connected now. Press d on it to disconnect it first."
            ));
        }
        if let Some(old) = known
            && self.replace.as_deref() != Some(name.as_str())
        {
            self.replace = Some(name.clone());
            return Err(format!(
                "config.yaml already runs {} for {name}. Press enter again to replace it.",
                old.join(" ")
            ));
        }
        Ok(Submit::Save(name, command))
    }
}

/// An answer to a prompt, kept out of debug output since it may be a
/// passphrase.
#[derive(Clone, PartialEq, Eq)]
pub struct Answer(pub String);

impl std::fmt::Debug for Answer {
    /// The length only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Answer({} bytes)", self.0.len())
    }
}

/// A question the box's link asks while the modal is attached.
#[derive(Clone, PartialEq, Eq)]
pub struct Prompt {
    /// Where the answer goes, as the daemon's `remote.prompt` names it.
    pub stream_id: u64,
    /// `code` for a pairing code, `secret` for a passphrase or password
    /// (shown masked), anything else for text such as a host key's yes.
    pub kind: String,
    /// What ssh or the daemon asked, cleaned, one entry per line.
    pub text: Vec<String>,
    pub input: String,
    /// The link's attempt when it asked; a state of another attempt means
    /// the question is gone.
    pub attempt: Option<u64>,
}

impl std::fmt::Debug for Prompt {
    /// Everything but what was typed, which may be a passphrase.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompt")
            .field("stream_id", &self.stream_id)
            .field("kind", &self.kind)
            .field("text", &self.text)
            .field(
                "input",
                &format_args!("{} chars", self.input.chars().count()),
            )
            .field("attempt", &self.attempt)
            .finish()
    }
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
    /// Lines of the notices ssh printed on the way, such as a host key it added.
    pub notes: Vec<String>,
}

impl Link {
    /// Takes a `remote.state` event's data.
    pub fn update(&mut self, data: &Value) {
        let text = |key: &str| data.get(key).and_then(Value::as_str).map(text::clean);
        self.state = text("state").unwrap_or_default();
        self.attempt = data.get("attempt").and_then(Value::as_u64);
        self.error = text("error")
            .map(|error| error.replace('`', ""))
            .filter(|error| !error.is_empty());
        self.retry_in_ms = data.get("retry_in_ms").and_then(Value::as_u64);
        self.age = 0;
    }

    /// The state in words: `connecting…`, `next try in 4s`.
    pub fn words(&self) -> String {
        match self.state.as_str() {
            "" | "connecting" => match self.attempt {
                Some(attempt) if attempt > 1 => format!("connecting, try {attempt}…"),
                _ => "connecting…".into(),
            },
            "reconnecting" => next_try(self.retry_in_ms, self.age),
            "needs_auth" => "waiting for a sign-in…".into(),
            "needs_pairing" => "waiting for a pairing code…".into(),
            "connected" => "connected".into(),
            state => text::clean(&state.replace('_', " ")),
        }
    }
}

/// `next try in 4s`, or `trying again now` once the wait is over: how long a
/// link waits for its next attempt, `retry_in_ms` from `age` seconds ago.
pub fn next_try(retry_in_ms: Option<u64>, age: u64) -> String {
    let left = retry_in_ms.map(|ms| ms.saturating_sub(age * 1000).div_ceil(1000));
    match left {
        Some(seconds) if seconds > 0 => format!("next try in {seconds}s"),
        _ => "trying again now".into(),
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
    /// The `connect` pairs by ticket; [`ConnectDialog::host`] stays empty
    /// until its first state names the box.
    pub by_ticket: bool,
    /// What the pairing answered, once the box has a name here.
    pub paired: Option<Paired>,
}

/// `paired` of a `connect` by ticket: the name the box is kept under here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Paired {
    pub name: String,
    /// This machine was paired with the box before and kept that name.
    pub kept: bool,
    /// A typed name was ignored for the kept one.
    pub name_ignored: bool,
}

impl Paired {
    /// `paired` of a `remote.state` event or a `connect` answer.
    pub fn of(data: &Value) -> Option<Self> {
        let paired = data.get("paired")?;
        let flag = |key: &str| paired.get(key).and_then(Value::as_bool) == Some(true);
        Some(Self {
            name: text::clean(paired.get("name")?.as_str()?),
            kept: flag("kept"),
            name_ignored: flag("name_ignored"),
        })
    }

    /// The name the box is kept under, in a sentence: `This machine keeps
    /// the box as acme.`, and when an earlier pairing's name was kept, so.
    pub fn words(&self) -> String {
        let name = &self.name;
        match (self.kept, self.name_ignored) {
            (true, true) => format!(
                "This machine was paired with the box before and keeps its name {name}, so the \
                 name you typed was ignored."
            ),
            (true, false) => {
                format!("This machine was paired with the box before and keeps its name {name}.")
            }
            (false, _) => format!("Paired. This machine keeps the box as {name}."),
        }
    }
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
    /// Pair with the box in `ticket` under `name`, then connect it.
    Pair {
        ticket: TicketText,
        name: Option<String>,
    },
    /// Send `answer` for the prompt on `stream_id`.
    Answer {
        stream_id: u64,
        answer: Answer,
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
            by_ticket: false,
            paired: None,
        }
    }

    /// Whether Enter on a failed pairing goes back to the form for the
    /// ticket again: the box has no name here to connect to yet.
    pub fn pastes_again(&self) -> bool {
        self.by_ticket && self.paired.is_none()
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

    /// Whether a click outside the modal is ignored rather than closing it:
    /// while it saves, which takes a moment, and while it asks something,
    /// which a stray click must not cancel.
    pub fn holds_clicks(&self) -> bool {
        matches!(self.step, Step::Saving | Step::Prompt(_))
    }

    /// Takes a key.
    pub fn key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Saving takes milliseconds, and then the box connects.
        if self.step == Step::Saving {
            return Action::Stay;
        }
        if key.code == KeyCode::Esc {
            return Action::Close;
        }
        match &mut self.step {
            Step::Form => self.form_key(key, ctrl),
            Step::Prompt(prompt) => match key.code {
                KeyCode::Enter => Action::Answer {
                    stream_id: prompt.stream_id,
                    // An empty frame cancels, so an empty answer is a newline.
                    answer: Answer(format!("{}\n", prompt.input)),
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
                KeyCode::Enter if self.pastes_again() => {
                    self.step = Step::Form;
                    self.form.field = Field::Ticket;
                    self.form.error = None;
                    Action::Stay
                }
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
                form.reach = form
                    .reach
                    .step(if key.code == KeyCode::Left { -1 } else { 1 });
                form.error = None;
                form.replace = None;
            }
            KeyCode::Enter => match form.submit() {
                Ok(submit) => {
                    form.error = None;
                    return match submit {
                        Submit::Connect(host) => {
                            self.host = host;
                            self.by_ticket = false;
                            Action::Connect
                        }
                        Submit::Save(host, command) => {
                            self.host = host;
                            self.by_ticket = false;
                            Action::Save(command)
                        }
                        Submit::Pair(ticket, name) => {
                            self.host = String::new();
                            self.by_ticket = true;
                            self.paired = None;
                            Action::Pair { ticket, name }
                        }
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
    /// are left out so a paste never sends. A ticket pasted anywhere on the
    /// form goes to the ticket row, so it is never shown in the name.
    pub fn paste(&mut self, pasted: &str) {
        let pasted: String = pasted.chars().filter(|c| !c.is_control()).collect();
        match &mut self.step {
            Step::Form if devlanes_proto::remote::holds_ticket(&pasted_ticket(&pasted)) => {
                let form = &mut self.form;
                form.reach = Reach::Ticket;
                form.field = Field::Ticket;
                form.error = None;
                form.ticket = TicketText(pasted_ticket(&pasted));
            }
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
        let kind = data
            .get("kind")
            .and_then(Value::as_str)
            .map(text::clean)
            .unwrap_or_default();
        let asked = text::clean_lines(data.get("prompt").and_then(Value::as_str).unwrap_or(""));
        if kind == "notify" {
            self.link.notes.extend(asked);
            return Some(stream_id);
        }
        self.step = Step::Prompt(Prompt {
            stream_id,
            kind,
            text: asked,
            input: String::new(),
            attempt: self.link.attempt,
        });
        None
    }

    /// Whether a `remote.state` event's data is about this modal's box. A
    /// pairing by ticket learns the box's name from its first state, which
    /// carries `paired`.
    pub fn follows(&mut self, data: &Value) -> bool {
        if self.by_ticket
            && self.paired.is_none()
            && let Some(paired) = Paired::of(data)
        {
            // The event's own `host` is the name the link goes by.
            self.host = data
                .get("host")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            self.paired = Some(paired);
        }
        !self.host.is_empty() && data.get("host").and_then(Value::as_str) == Some(&self.host)
    }

    /// Takes a `remote.state` event's data. A question stays open while the
    /// attempt that asked it goes on, so a state sent late for it does not
    /// take the question away.
    pub fn state(&mut self, data: &Value) {
        self.link.update(data);
        if let Step::Prompt(prompt) = &self.step
            && (self.link.state != "connecting" || self.link.attempt != prompt.attempt)
        {
            self.step = Step::Waiting;
        }
    }
}

impl Default for ConnectDialog {
    /// The empty form, as [`ConnectDialog::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// `line` split into a program and its arguments, the way a shell splits
/// words: spaces separate them, single quotes keep everything and double
/// quotes keep spaces. A backslash is kept as it is, so Windows paths
/// (`C:\tools\x.exe`, `\\server\share`) type as they are, except that `\"`
/// is a quote. Inside double quotes a `\"` at the end of a word closes the
/// quotes instead, so `"C:\dir\"` is `C:\dir\`.
pub(crate) fn split_command(line: &str) -> Result<Vec<String>, String> {
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
                        Some('\\') if chars.peek() == Some(&'"') => {
                            let mut ahead = chars.clone();
                            ahead.next();
                            // `\"` before the end of the word ends the quotes.
                            if ahead.peek().is_none_or(|next| next.is_whitespace()) {
                                word.push('\\');
                                chars.next();
                                break;
                            }
                            word.extend(chars.next());
                        }
                        Some(c) => word.push(c),
                        None => return Err(unclosed()),
                    }
                }
            }
            '\\' if chars.peek() == Some(&'"') => {
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

/// The pairing modal's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairState {
    /// `remote.pair` is out.
    Loading,
    /// What another machine pairs with: a ticket, a code or both.
    Offer(Offer),
    /// Wrong codes paused pairing for `wait_ms` from when it arrived; only
    /// a daemon older than tickets answers so.
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

/// What `remote.pair` gave, as the modal shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Offer {
    pub code: Option<String>,
    /// How long the code was valid for when it arrived.
    pub valid_ms: Option<u64>,
    pub ticket: Option<OfferedTicket>,
    /// Why there is no ticket, while there is none.
    pub no_ticket: Option<NoTicket>,
    /// Wrong codes paused codes for this long from when the answer came.
    pub code_paused_ms: Option<u64>,
}

/// A ticket for another machine and how long it is valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferedTicket {
    pub text: TicketText,
    pub valid_ms: u64,
    /// [`PairDialog::age`] when it arrived: a ticket asked for alone comes
    /// after the code.
    pub since: u64,
}

/// Why the modal shows no ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoTicket {
    /// The daemon answered without a ticket and without saying why, as
    /// daemons from before tickets do.
    OldDaemon,
    /// The ticket was not ready within the daemon's short wait, as after a
    /// start or after the peer-to-peer networking stopped while idle.
    NotReady,
    /// The ticket alone is being asked for, which waits for it.
    Fetching,
    /// The daemon could not make one: its code and words.
    Failed { code: String, message: String },
}

/// The pairing modal (`p` in the boxes view).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairDialog {
    pub state: PairState,
    /// Seconds since the state arrived, for the countdowns.
    pub age: u64,
    /// Which request's answers this modal takes.
    pub generation: u64,
}

/// `remote.pair`'s `ticket` in `value`, valid for its `valid_ms` from now.
fn ticket_of(value: &Value, since: u64) -> Option<OfferedTicket> {
    let text = value.get("ticket").and_then(Value::as_str)?;
    Some(OfferedTicket {
        text: TicketText(text::clean(text)),
        valid_ms: value
            .get("valid_ms")
            .and_then(Value::as_u64)
            .unwrap_or(devlanes_proto::remote::PAIRING_CODE_TTL_MS),
        since,
    })
}

/// Why a ticket failed, from `{code, message}` (`ticket_error`, or the
/// error of a request).
fn no_ticket(code: &str, message: &str) -> NoTicket {
    if code == "ticket_not_ready" {
        return NoTicket::NotReady;
    }
    NoTicket::Failed {
        code: text::clean(code),
        message: text::clean(message).replace('`', ""),
    }
}

impl PairDialog {
    /// Waiting for a code.
    pub fn loading() -> Self {
        Self {
            state: PairState::Loading,
            age: 0,
            generation: 0,
        }
    }

    /// Whether a request of this modal is out: the first one, or `t`'s.
    pub fn asking(&self) -> bool {
        match &self.state {
            PairState::Loading => true,
            PairState::Offer(offer) => offer.no_ticket == Some(NoTicket::Fetching),
            _ => false,
        }
    }

    /// Takes `remote.pair`'s answer to a request for a ticket and a code. A
    /// code always shows, with a note for a ticket that is missing; an
    /// answer with neither is an error. `connections_off` and an old
    /// daemon's `pairing_paused` are states of their own, other errors are
    /// shown in words.
    pub fn answered(&mut self, result: Result<Value, crate::ApiError>) {
        self.age = 0;
        // An answer with neither while codes are paused reads as the CLI's
        // `pairing_paused`; with neither and no pause, as its empty answer.
        let result = match result {
            Ok(value) => match devlanes_cli::nothing_minted(&value) {
                Some(error) if error.code != "pairing_empty" => Err(crate::ApiError::Rpc(error)),
                _ => Ok(value),
            },
            error => error,
        };
        self.state = match result {
            Ok(value) => {
                let code = value.get("code").and_then(Value::as_str).map(text::clean);
                let ticket = ticket_of(&value, 0);
                let reason = value.get("ticket_error").map(|error| {
                    let field = |key| error.get(key).and_then(Value::as_str).unwrap_or_default();
                    no_ticket(field("code"), field("message"))
                });
                if code.is_none() && ticket.is_none() {
                    PairState::Failed(Worded::local("the daemon sent neither a ticket nor a code"))
                } else {
                    PairState::Offer(Offer {
                        no_ticket: if ticket.is_some() {
                            None
                        } else {
                            Some(reason.unwrap_or(NoTicket::OldDaemon))
                        },
                        valid_ms: code.as_ref().map(|_| {
                            value
                                .get("valid_ms")
                                .and_then(Value::as_u64)
                                .unwrap_or(devlanes_proto::remote::PAIRING_CODE_TTL_MS)
                        }),
                        code,
                        ticket,
                        code_paused_ms: value.get("code_paused_ms").and_then(Value::as_u64),
                    })
                }
            }
            // Codes are paused and the ticket failed too, so nothing went
            // out; the ticket alone may still come.
            Err(crate::ApiError::Rpc(error))
                if error.data.get("code_paused_ms").is_some()
                    && matches!(
                        error.code.as_str(),
                        "ticket_not_ready" | "p2p_unavailable" | "random_failed"
                    ) =>
            {
                let message = error
                    .data
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                PairState::Offer(Offer {
                    no_ticket: Some(no_ticket(&error.code, message)),
                    code_paused_ms: error.data.get("code_paused_ms").and_then(Value::as_u64),
                    ..Offer::default()
                })
            }
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

    /// Milliseconds left of the ticket's validity.
    pub fn ticket_left(&self, ticket: &OfferedTicket) -> u64 {
        ticket
            .valid_ms
            .saturating_sub(self.age.saturating_sub(ticket.since) * 1000)
    }

    /// The code shown, while it has not expired.
    pub fn code(&self) -> Option<&str> {
        match &self.state {
            PairState::Offer(Offer {
                code: Some(code),
                valid_ms,
                ..
            }) if valid_ms.is_none_or(|valid| self.left(valid) > 0) => Some(code),
            _ => None,
        }
    }

    /// The ticket shown, while it has not expired.
    pub fn ticket(&self) -> Option<&str> {
        match &self.state {
            PairState::Offer(Offer {
                ticket: Some(ticket),
                ..
            }) if self.ticket_left(ticket) > 0 => Some(ticket.text.expose()),
            _ => None,
        }
    }

    /// Whether `t` asks for the ticket alone: while there is none and the
    /// daemon may still make one. A daemon older than tickets or built
    /// without them never does.
    pub fn may_fetch_ticket(&self) -> bool {
        let PairState::Offer(offer) = &self.state else {
            return false;
        };
        match &offer.no_ticket {
            Some(NoTicket::NotReady) => true,
            Some(NoTicket::Failed { code, .. }) => code != "not_supported_yet",
            _ => false,
        }
    }

    /// `t`: marks the ticket as asked for; true when the request should go.
    pub fn fetch_ticket(&mut self) -> bool {
        if !self.may_fetch_ticket() {
            return false;
        }
        if let PairState::Offer(offer) = &mut self.state {
            offer.no_ticket = Some(NoTicket::Fetching);
        }
        true
    }

    /// Takes the answer to a request for the ticket alone, which only the
    /// modal that asked waits for. The code shown stays as it is, so an
    /// error never takes its place.
    pub fn ticket_answered(&mut self, result: Result<Value, crate::ApiError>) {
        let age = self.age;
        let PairState::Offer(offer) = &mut self.state else {
            return;
        };
        if offer.no_ticket != Some(NoTicket::Fetching) {
            return;
        }
        match result {
            Ok(value) => match ticket_of(&value, age) {
                Some(ticket) => {
                    offer.ticket = Some(ticket);
                    offer.no_ticket = None;
                }
                None => offer.no_ticket = Some(NoTicket::OldDaemon),
            },
            Err(crate::ApiError::Rpc(error)) if error.data.get("message").is_some() => {
                let message = error.data["message"].as_str().unwrap_or_default();
                offer.no_ticket = Some(no_ticket(&error.code, message));
            }
            Err(error) => {
                let worded = crate::words::api(&error);
                offer.no_ticket = Some(NoTicket::Failed {
                    code: worded.code.unwrap_or_default(),
                    message: worded.summary,
                });
            }
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

    /// Words split on spaces and quotes group them. Backslashes stay as they
    /// are, so Windows and UNC paths type as they are, except before a quote.
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
            split(r#"C:\tools\box.exe \\server\share\x.exe a\ b say \"hi\" ''"#),
            [
                r"C:\tools\box.exe",
                r"\\server\share\x.exe",
                r"a\",
                "b",
                "say",
                r#""hi""#,
                ""
            ]
        );
        assert_eq!(
            split(r#""C:\Program Files\devlanes\" "say \"hi\"" "\\server\share""#),
            [
                r"C:\Program Files\devlanes\",
                r#"say "hi""#,
                r"\\server\share"
            ]
        );
        assert_eq!(split("   "), Vec::<String>::new());
        assert!(split_command("docker 'exec").is_err());
        assert!(split_command("docker \"exec").is_err());
    }

    /// The form says what is wrong in words; a box reached by a command
    /// needs a name config.yaml can hold.
    #[test]
    fn the_form_checks_what_was_typed() {
        let form = |reach, name: &str, command: &str| {
            Form {
                reach,
                name: name.into(),
                command: command.into(),
                ..Form::default()
            }
            .submit()
        };
        assert_eq!(
            form(Reach::Ssh, " me@203.0.113.4 ", ""),
            Ok(Submit::Connect("me@203.0.113.4".into()))
        );
        assert!(form(Reach::Ssh, "", "").is_err());
        assert!(form(Reach::Ssh, "-oProxyCommand=x", "").is_err());
        assert!(form(Reach::Ssh, "two words", "").is_err());
        assert_eq!(
            form(Reach::Command, "acme", "wsl.exe -d Ubuntu"),
            Ok(Submit::Save(
                "acme".into(),
                ["wsl.exe", "-d", "Ubuntu"].map(String::from).to_vec()
            ))
        );
        assert!(form(Reach::Command, "me@box", "x").is_err());
        assert!(form(Reach::Command, "acme", "  ").is_err());
    }

    /// A command already in the config is asked about before it is replaced,
    /// the same command just connects, a box with a link keeps its command
    /// until it is disconnected, and an SSH name with a command says so.
    #[test]
    fn the_form_checks_the_config_and_live_boxes() {
        let docker = ["docker", "exec", "-i", "acme"].map(String::from).to_vec();
        let mut form = Form {
            reach: Reach::Command,
            name: "acme".into(),
            command: "wsl.exe -d Ubuntu".into(),
            known: BTreeMap::from([("acme".to_owned(), docker.clone())]),
            ..Form::default()
        };
        let asked = form.submit().expect_err("asks first");
        assert!(
            asked.contains("already runs docker exec -i acme"),
            "{asked}"
        );
        assert!(
            asked.contains("Press enter again to replace it."),
            "{asked}"
        );
        assert!(matches!(form.submit(), Ok(Submit::Save(..))));
        form.command = "docker exec -i acme".into();
        assert_eq!(form.submit(), Ok(Submit::Connect("acme".into())));
        form.command = "wsl.exe -d Ubuntu".into();
        form.live = vec!["acme".into()];
        let live = form.submit().expect_err("connected");
        assert!(
            live.contains("Press d on it to disconnect it first."),
            "{live}"
        );
        form.reach = Reach::Ssh;
        assert_eq!(form.configured(), Some(&docker));
    }

    /// Tab goes through the rows, space switches how the box is reached,
    /// letters go to the field with the focus and Enter sends the form.
    #[test]
    fn form_keys() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let mut dialog = ConnectDialog::new();
        assert_eq!(dialog.form.field, Field::Ticket);
        dialog.key(key(KeyCode::BackTab));
        assert_eq!(dialog.form.field, Field::Reach);
        dialog.key(key(KeyCode::Left));
        assert_eq!(dialog.form.reach, Reach::Command);
        dialog.key(key(KeyCode::Tab));
        for c in "acme".chars() {
            dialog.key(key(KeyCode::Char(c)));
        }
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
        // Esc does nothing while the command is saved.
        dialog.step = Step::Saving;
        assert_eq!(dialog.key(key(KeyCode::Esc)), Action::Stay);
        dialog.step = Step::Form;
        assert_eq!(dialog.key(key(KeyCode::Esc)), Action::Close);
    }

    /// A question becomes the prompt step with its lines, its answer ends in
    /// a newline and stays out of debug output; a notice is kept and
    /// answered at once.
    #[test]
    fn prompts_and_notices() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let mut dialog = ConnectDialog::following("hetzner");
        let notice =
            json!({"kind": "notify", "prompt": "Warning: added the host key", "stream_id": 3});
        assert_eq!(dialog.prompt(&notice), Some(3));
        assert_eq!(dialog.link.notes, ["Warning: added the host key"]);
        assert_eq!(dialog.step, Step::Waiting);
        let host_key = json!({"kind": "confirm", "stream_id": 5,
            "prompt": "The authenticity of host 'hetzner' can't be established.\nAre you sure (yes/no)? "});
        assert_eq!(dialog.prompt(&host_key), None);
        assert!(matches!(&dialog.step, Step::Prompt(p) if p.text.len() == 2));
        let secret = json!({"kind": "secret", "prompt": "Enter passphrase:", "stream_id": 4});
        assert_eq!(dialog.prompt(&secret), None);
        dialog.key(key(KeyCode::Char('h')));
        dialog.key(key(KeyCode::Char('i')));
        assert!(matches!(&dialog.step, Step::Prompt(p) if p.masked() && p.input == "hi"));
        assert!(!format!("{:?}", dialog.step).contains("hi\""));
        let answer = dialog.key(key(KeyCode::Enter));
        assert_eq!(
            answer,
            Action::Answer {
                stream_id: 4,
                answer: Answer("hi\n".into())
            }
        );
        assert!(!format!("{answer:?}").contains("hi"), "{answer:?}");
    }

    /// A state of the attempt that asked keeps the question open; a state of
    /// another attempt, or one that is not connecting, takes it away.
    #[test]
    fn late_states_keep_the_question() {
        let mut dialog = ConnectDialog::following("hetzner");
        let state = |state: &str, attempt: u64| json!({"host": "hetzner", "state": state, "attempt": attempt});
        dialog.state(&state("connecting", 1));
        dialog.prompt(&json!({"kind": "secret", "prompt": "Password:", "stream_id": 4}));
        dialog.state(&state("connecting", 1));
        assert!(matches!(dialog.step, Step::Prompt(_)), "the same attempt");
        dialog.state(&state("connecting", 2));
        assert_eq!(dialog.step, Step::Waiting, "another attempt");
        dialog.prompt(&json!({"kind": "secret", "prompt": "Password:", "stream_id": 5}));
        dialog.state(&json!({"host": "hetzner", "state": "reconnecting", "retry_in_ms": 500}));
        assert_eq!(dialog.step, Step::Waiting, "the attempt ended");
    }

    /// The pairing modal counts its code down from when it arrived and has
    /// states of its own for a pause and for connections being off.
    #[test]
    fn pairing_states() {
        let mut dialog = PairDialog::loading();
        dialog.answered(Ok(json!({
            "code": "K7QM-3XWD",
            "expires_at_ms": 1_793_000_600_000_u64,
            "valid_ms": devlanes_proto::remote::PAIRING_CODE_TTL_MS,
        })));
        dialog.age = 2;
        assert_eq!(countdown(dialog.left(600_000)), "9m 58s");
        assert_eq!(dialog.code(), Some("K7QM-3XWD"));
        dialog.age = 600;
        assert_eq!(dialog.code(), None, "an expired code is not offered");

        // A ticket that was not ready leaves the code in place, and `t`
        // asks for the ticket alone, whose countdown starts when it comes;
        // a failure of that request keeps the code too.
        let code_and = |extra: Value| {
            let mut answer = json!({"code": "K7QM-3XWD", "valid_ms": 600_000});
            if let (Some(answer), Some(extra)) = (answer.as_object_mut(), extra.as_object()) {
                answer.extend(extra.clone());
            }
            let mut dialog = PairDialog::loading();
            dialog.answered(Ok(answer));
            dialog
        };
        let mut dialog = code_and(json!({"ticket_error": {"code": "ticket_not_ready",
            "message": "this machine was still reaching its relay"}}));
        assert_eq!(dialog.code(), Some("K7QM-3XWD"));
        assert!(dialog.fetch_ticket());
        assert!(!dialog.fetch_ticket(), "one request per t");
        dialog.age = 30;
        dialog.ticket_answered(Ok(json!({"ticket": "dlp1abc", "valid_ms": 600_000})));
        assert_eq!(dialog.ticket(), Some("dlp1abc"));
        dialog.age = 600;
        assert_eq!(dialog.code(), None);
        assert_eq!(dialog.ticket(), Some("dlp1abc"), "the ticket came later");
        let mut dialog = code_and(json!({"ticket_error": {"code": "ticket_not_ready",
            "message": "not yet"}}));
        dialog.fetch_ticket();
        dialog.ticket_answered(Err(crate::ApiError::Rpc(devlanes_proto::RpcError::new(
            "p2p_unavailable",
            json!({"message": "this machine has no network address"}),
            1,
        ))));
        assert_eq!(dialog.code(), Some("K7QM-3XWD"));
        assert!(matches!(
            &dialog.state,
            PairState::Offer(Offer { no_ticket: Some(NoTicket::Failed { message, .. }), .. })
                if message == "this machine has no network address"
        ));
        // Without a ticket or a reason, the daemon is older than tickets;
        // built without them, it never makes one.
        let dialog = code_and(json!({}));
        assert!(matches!(
            &dialog.state,
            PairState::Offer(Offer {
                no_ticket: Some(NoTicket::OldDaemon),
                ..
            })
        ));
        assert!(!dialog.may_fetch_ticket());
        let dialog = code_and(json!({"ticket_error": {"code": "not_supported_yet",
            "message": "pairing by ticket is not supported yet"}}));
        assert!(!dialog.may_fetch_ticket());
        // Paused codes: the ticket alone, or nothing but the wait and `t`.
        let mut dialog = PairDialog::loading();
        dialog.answered(Ok(
            json!({"ticket": "dlp1abc", "valid_ms": 600_000, "code_paused_ms": 252_000}),
        ));
        assert_eq!((dialog.ticket(), dialog.code()), (Some("dlp1abc"), None));
        let mut dialog = PairDialog::loading();
        dialog.answered(Err(crate::ApiError::Rpc(devlanes_proto::RpcError::new(
            "ticket_not_ready",
            json!({"message": "not yet", "code_paused_ms": 252_000}),
            1,
        ))));
        assert!(dialog.may_fetch_ticket());
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
        assert_eq!(next_try(Some(4_000), 0), "next try in 4s");
        assert_eq!(next_try(Some(4_000), 5), "trying again now");
    }
}
