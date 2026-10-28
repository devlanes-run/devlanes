//! Boxes on screen: the pane of a box lane, which shows a unit's details
//! (state, ports with their mirror state, hostnames with the URL that opens
//! here, syncs) where a local unit has its terminal, and the boxes view (`B`)
//! with every box's link and what it mirrors, then this machine as a box
//! with the clients that connect to it.

use devlanes_proto::LaneSummary;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::list::{container_look, lane_summary};
use super::pane::{LABEL, banner, details, field, render_box, sentence, title, unit_facts_title};
use super::{draw_box, fit, spread, unit_look};
use crate::app::{App, BoxPart, Grey};
use crate::boxes::{BoxHost, RegistryUnit};
use crate::clients::{Client, short_id};
use crate::text::{self, clean, clip};
use crate::theme::{Glyph, Theme, Tone};
use crate::tree::{self, Role, RowId};

/// The pane for the selected row of box lane `box_lane` (`acme@1`) of `host`.
pub(super) fn pane(
    app: &App,
    host: &BoxHost,
    box_lane: &str,
    lane: &LaneSummary,
    frame: &mut Frame,
    boxed: Rect,
    area: Rect,
) {
    let theme = &app.theme;
    let width = usize::from(area.width).max(1);
    let right = vec![Span::styled(
        format!("on {}", clean(&host.host)),
        theme.quiet(),
    )];
    let mut lines = notes(app, host, box_lane, &lane.reference, width);
    match app.selected_id().cloned() {
        Some(RowId::Unit(name)) => {
            let unit = lane.units.iter().find(|u| u.name == name);
            // A box port is no address here unless the unit's URLs list it.
            let (label, facts) = match unit {
                Some(unit) if unit.urls.is_empty() => {
                    let look = unit_look(theme, unit, app.spin);
                    let facts = vec![Span::styled(look.word, theme.tone(look.tone))];
                    (tree::short_name(unit).to_owned(), facts)
                }
                Some(unit) => (
                    tree::short_name(unit).to_owned(),
                    unit_facts_title(app, unit),
                ),
                None => (name.clone(), Vec::new()),
            };
            render_box(app, frame, boxed, title(&label, &name, app), facts, right);
            if let (Some(unit), Some(registered)) = (unit, host.unit(box_lane, &name)) {
                let look = unit_look(theme, unit, app.spin);
                lines.extend(field(
                    theme,
                    "state",
                    &format!("{} on the box", look.word),
                    width,
                    theme.tone(look.word_tone),
                ));
                if tree::role(unit) == Role::Task {
                    lines.extend(sentence(
                        "The box reports a task's state only.",
                        width,
                        Vec::new(),
                        theme.quiet(),
                    ));
                }
                lines.extend(endpoint_lines(app, host, box_lane, registered, width));
            }
            let command = format!("devlanes logs {box_lane}:{name}");
            let text = format!("Read-only here. On {}: {}", host.host, host.there(&command));
            lines.extend(read_only(theme, &text, width));
        }
        Some(RowId::Container(name)) => {
            let container = lane.containers.iter().find(|c| c.name == name);
            let facts = container
                .map(|c| {
                    let (_, tone, word) = container_look(app, Some(lane), c);
                    vec![Span::styled(word, theme.tone(tone))]
                })
                .unwrap_or_default();
            render_box(app, frame, boxed, title(&name, "", app), facts, right);
            if let Some(registered) = host.unit(box_lane, &name) {
                lines.extend(endpoint_lines(app, host, box_lane, registered, width));
            }
            let command = format!("devlanes status {box_lane}");
            let text = format!("Read-only here. On {}: {}", host.host, host.there(&command));
            lines.extend(read_only(theme, &text, width));
        }
        Some(RowId::Section(_) | RowId::Group(..)) => {
            let label = match app.selected_id() {
                Some(RowId::Section(section)) => section.name().to_owned(),
                Some(RowId::Group(_, group)) => clean(group),
                _ => String::new(),
            };
            render_box(app, frame, boxed, title(&label, "", app), Vec::new(), right);
            let members = app
                .rows()
                .into_iter()
                .find(|row| Some(&row.id) == app.selected_id())
                .map(|row| row.members)
                .unwrap_or_default();
            lines.extend(members_table(app, lane, &members, width));
        }
        _ => {
            render_box(
                app,
                frame,
                boxed,
                title(box_lane, "", app),
                lane_summary(app, lane),
                right,
            );
            lines.extend(lane_lines(app, host, box_lane, lane, width));
            let command = format!("devlanes status {box_lane}");
            let text = format!("Read-only here. On {}: {}", host.host, host.there(&command));
            lines.extend(read_only(theme, &text, width));
        }
    }
    details(frame, lines, area, app);
}

/// Banners above a box lane's details: a key that just did nothing here and
/// what to run on the box instead, a link that is down, so what shows is the
/// last known state, and a lane this machine does not mirror. `reference`
/// is the lane's in the TUI, `acme@1 @hetzner`.
fn notes(
    app: &App,
    host: &BoxHost,
    box_lane: &str,
    reference: &str,
    width: usize,
) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let attention = |text: &str, rest: &[String]| {
        let mut lines = banner(theme, Glyph::Attention, Tone::Attention, text, rest, width);
        lines.push(Line::from(""));
        lines
    };
    let mut lines = Vec::new();
    if let Some((selected, text)) = &app.refusal
        && app.selected.as_ref() == Some(selected)
    {
        lines.extend(attention(text, &[]));
    }
    let name = clean(&host.host);
    let down = match host.state.as_str() {
        "connected" => None,
        "needs_auth" => Some((
            format!("{name} needs you to sign in again, so this is its last known state."),
            vec![format!("Press B, then enter on {name} to sign in.")],
        )),
        "needs_pairing" => Some((
            format!("{name} needs a new pairing code, so this is its last known state."),
            vec![format!(
                "Run devlanes daemon pair on {name}. Then press B and enter on {name}."
            )],
        )),
        "reconnecting" | "connecting" => Some((
            format!("{name} is reconnecting, so this is its last known state."),
            Vec::new(),
        )),
        state => Some((
            format!(
                "{name} is {}, so this is its last known state.",
                clean(state)
            ),
            Vec::new(),
        )),
    };
    if let Some((down, rest)) = down {
        lines.extend(attention(&down, &rest));
    }
    if let Some(grey) = app.grey(reference) {
        match grey_note(&grey) {
            Some(note) => lines.extend(attention(&note, &[])),
            // A daemon without `remote.use` says why, and nothing switches it.
            None => {
                let reason = host
                    .skipped(box_lane)
                    .map_or_else(|| clean(&grey.reason), |skipped| skip_reason(app, skipped));
                lines.extend(attention(
                    &format!("Not mirrored here because {reason}."),
                    &["Its ports and hostnames are mirrored once that changes.".to_owned()],
                ));
            }
        }
    }
    lines
}

/// The first line of a grey lane's pane: what serves its hostnames instead
/// and how to use this one. `None` from daemons without `remote.use`.
pub(super) fn grey_note(grey: &Grey) -> Option<String> {
    let serving = clean(&grey.serving);
    match (grey.cause.as_deref()?, &grey.winner) {
        ("local_running", _) => Some(format!(
            "Not mirrored. {serving} runs on this machine. Press m to stop it and use this one."
        )),
        // This machine's own lane, which a chosen box lane took over.
        (_, Some(winner)) if grey.host.is_none() => Some(format!(
            "{}'s {serving} is in use instead. Press m to use this one.",
            clean(winner)
        )),
        (_, Some(winner)) if grey.can_use => Some(format!(
            "Not mirrored. {}'s {serving} is in use. Press m to use this one.",
            clean(winner)
        )),
        (_, None) if grey.can_use => Some(format!(
            "Not mirrored. This machine's {serving} is in use. Press m to use this one."
        )),
        _ => None,
    }
}

/// A skipped lane's row in the boxes view: what serves its hostnames and
/// what `m` on the row does. `None` from daemons without `remote.use`.
fn skipped_words(skipped: &crate::boxes::SkippedLane) -> Option<String> {
    let cause = skipped.cause.as_deref()?;
    let lane = clean(&skipped.lane);
    let serving = clean(skipped.serving());
    match (cause, skipped.served_by()) {
        // `acme@0: acme@0 runs on this machine` would name the lane twice.
        ("local_running", _) if serving == lane => Some(format!(
            "{lane} also runs on this machine. Press m to stop it and use this one."
        )),
        ("local_running", _) => Some(format!(
            "{lane}: {serving} runs on this machine. Press m to stop it and use this one."
        )),
        (_, Some(winner)) if skipped.can_use => Some(format!(
            "{lane}: {}'s {serving} is in use. Press m to use this one.",
            clean(winner)
        )),
        (_, None) if skipped.can_use => Some(format!(
            "{lane}: this machine's {serving} is in use. Press m to use this one."
        )),
        _ => None,
    }
}

/// Why a box lane is skipped, naming the local lane that serves its
/// hostnames when one does: local lanes win over every box, so that lane is
/// the one to stop.
fn skip_reason(app: &App, skipped: &crate::boxes::SkippedLane) -> String {
    match app.local_lane_serving(&skipped.hosts) {
        Some(local) if skipped.other.is_none() => {
            format!("this machine's {} is in use", clean(&local))
        }
        _ => clean(&skipped.reason),
    }
}

/// The last lines of a box pane: `Read-only here.` and where to act instead.
fn read_only(theme: &Theme, text: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("")];
    lines.extend(sentence(text, width, Vec::new(), theme.quiet()));
    lines
}

/// One row of a details table: `label`, then `lead` (a fixed column such as
/// a port), then `value` wrapped under itself.
fn row(
    theme: &Theme,
    label: &str,
    lead: &str,
    value: &str,
    style: Style,
    width: usize,
) -> Vec<Line<'static>> {
    let indent = LABEL + text::width(lead);
    let value_width = width.saturating_sub(indent).max(8);
    text::wrap(&clean(value), value_width)
        .into_iter()
        .enumerate()
        .map(|(index, part)| {
            let (label, lead) = if index == 0 {
                (label.to_owned(), lead.to_owned())
            } else {
                (String::new(), " ".repeat(text::width(lead)))
            };
            fit(
                vec![
                    Span::styled(format!("{label:<LABEL$}"), theme.quiet()),
                    Span::raw(lead),
                    Span::styled(part, style),
                ],
                width,
            )
        })
        .collect()
}

/// One entry of a group under a label: its text, style and whether the text
/// is a URL, which never wraps so it stays one link.
type Entry = (String, Style, bool);

/// `entries` under `label`, one per line, all in the same column: beside the
/// label when every URL fits there, else each on a line of its own from
/// column 2. A URL wider than even that is cut with `…`; other text wraps.
fn group(theme: &Theme, label: &str, entries: &[Entry], width: usize) -> Vec<Line<'static>> {
    let beside = entries
        .iter()
        .all(|(text, _, url)| !url || LABEL + text::width(text) <= width);
    let mut lines = Vec::new();
    if !beside && !entries.is_empty() {
        lines.push(Line::styled(label.to_owned(), theme.quiet()));
    }
    for (index, (text, style, url)) in entries.iter().enumerate() {
        if beside {
            let label = if index == 0 { label } else { "" };
            if *url {
                lines.push(fit(
                    vec![
                        Span::styled(format!("{label:<LABEL$}"), theme.quiet()),
                        Span::styled(clean(text), *style),
                    ],
                    width,
                ));
            } else {
                lines.extend(row(theme, label, "", text, *style, width));
            }
        } else if *url {
            lines.push(fit(
                vec![Span::raw("  "), Span::styled(clean(text), *style)],
                width,
            ));
        } else {
            lines.extend(sentence(text, width, vec![Span::raw("  ")], *style));
        }
    }
    lines
}

/// Ports with their mirror state, hostnames with the URL that opens them
/// here, the unit's syncs and whether it is hidden.
fn endpoint_lines(
    app: &App,
    host: &BoxHost,
    box_lane: &str,
    unit: &RegistryUnit,
    width: usize,
) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let mut lines = Vec::new();
    let names = unit
        .ports
        .iter()
        .map(|port| text::width(&port.name))
        .max()
        .unwrap_or(0)
        .clamp(4, 12);
    for (index, port) in unit.ports.iter().enumerate() {
        let label = if index == 0 { "ports" } else { "" };
        let lead = format!(
            "{:<names$} {:>5}  ",
            clip(&clean(&port.name), names),
            port.port
        );
        let (status, tone) = port_status(host, box_lane, unit, port.port);
        lines.extend(row(theme, label, &lead, &status, theme.tone(tone), width));
        let error = host
            .port(box_lane, &unit.key.name, port.port)
            .and_then(|p| p.last_error.as_ref());
        if let Some(error) = error {
            let pad = " ".repeat(text::width(&lead));
            // `the link to the box was lost: reset` reads
            // `last try: the link to the box was lost (reset)`.
            let message = clean(&error.message);
            let text = match message.split_once(": ") {
                Some((what, detail)) => format!("last try: {what} ({detail})"),
                None => format!("last try: {message}"),
            };
            lines.extend(row(
                theme,
                "",
                &pad,
                &text,
                theme.tone(Tone::Failure),
                width,
            ));
        }
    }
    let hosts: Vec<Entry> = unit
        .hosts
        .iter()
        .map(|entry| host_status(theme, host, box_lane, &entry.host))
        .collect();
    lines.extend(group(theme, "hosts", &hosts, width));
    for (index, sync) in host.syncs_of(box_lane, &unit.key.name).enumerate() {
        let label = if index == 0 { "syncs" } else { "" };
        let leaf = sync
            .from
            .rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or("sync")
            .to_owned();
        let (state, tone) = sync_state(&sync.state);
        let lead = format!("{}  ", clip(&clean(&leaf), 16));
        lines.extend(row(theme, label, &lead, state, theme.tone(tone), width));
        if let Some(error) = &sync.error {
            let pad = " ".repeat(text::width(&lead));
            lines.extend(row(
                theme,
                "",
                &pad,
                error,
                theme.tone(Tone::Failure),
                width,
            ));
        }
        let to = super::daemon::home_short(app, &sync.to);
        lines.extend(row(theme, "into", "", &to, theme.quiet(), width));
    }
    if unit.hidden {
        lines.extend(field(
            theme,
            "hidden",
            "yes, so it is never mirrored",
            width,
            theme.quiet(),
        ));
    }
    lines
}

/// What happens to box port `port` of `unit` on this machine.
fn port_status(host: &BoxHost, box_lane: &str, unit: &RegistryUnit, port: u16) -> (String, Tone) {
    if let Some(mirrored) = host.port(box_lane, &unit.key.name, port) {
        let local = mirrored.conflicts.iter().find_map(|c| c.local.as_deref());
        let holder = mirrored.conflicts.iter().find_map(|c| c.holder.as_deref());
        let state = mirrored
            .state
            .as_deref()
            .unwrap_or(if mirrored.addresses.is_empty() {
                "in_use"
            } else {
                "bound"
            });
        let relayed = if mirrored.relay.is_some() {
            ", but its hostnames still open it"
        } else {
            ""
        };
        return match (state, local) {
            ("local_lane", Some(local)) => (
                format!("not mirrored, kept for {}", clean(local)),
                Tone::Attention,
            ),
            ("bound", _) => {
                if mirrored.bound_v4() {
                    ("mirrored here".into(), Tone::Ok)
                } else {
                    ("mirrored here (IPv6 only)".into(), Tone::Ok)
                }
            }
            ("in_use", _) => {
                let text = match holder {
                    Some(holder) => format!("in use here by {}", clean(holder)),
                    None => "in use here".to_owned(),
                };
                (format!("{text}{relayed}"), Tone::Attention)
            }
            // The bind failed for another reason: the daemon's words say which.
            ("failed", _) => {
                let text = mirrored
                    .conflicts
                    .first()
                    .map_or_else(|| "cannot be bound here".to_owned(), |c| clean(&c.message));
                (format!("{text}{relayed}"), Tone::Attention)
            }
            _ => ("not bound here".into(), Tone::Quiet),
        };
    }
    if unit.hidden {
        return ("hidden on the box".into(), Tone::Quiet);
    }
    if host.skipped(box_lane).is_some() {
        return ("lane not mirrored".into(), Tone::Attention);
    }
    if !matches!(unit.state.as_str(), "ready" | "running") {
        return ("mirrored once it runs".into(), Tone::Quiet);
    }
    ("not mirrored".into(), Tone::Quiet)
}

/// A box hostname: the URL that opens it here, or why there is none; true
/// when the text is the URL alone.
fn host_status(theme: &Theme, host: &BoxHost, box_lane: &str, name: &str) -> (String, Style, bool) {
    let name = clean(name);
    if let Some(route) = host.route(&name) {
        // While the link is down the proxy answers with a page, not the box.
        let state = route
            .state
            .as_deref()
            .filter(|state| matches!(*state, "reconnecting" | "sign_in_needed" | "pairing_needed"))
            .map(|state| format!(" ({})", clean(&state.replace('_', " "))))
            .unwrap_or_default();
        return match &route.url {
            Some(url) if state.is_empty() => (clean(url), Style::new(), true),
            Some(url) => (format!("{}{state}", clean(url)), Style::new(), false),
            None => (format!("{name}{state}"), Style::new(), false),
        };
    }
    let why = if host.refused(&name) {
        suffixes(host).map_or_else(
            || "outside the names this machine routes".to_owned(),
            |names| format!("not routed here (not under {names})"),
        )
    } else if host.skipped(box_lane).is_some() {
        "not routed here (lane not mirrored)".to_owned()
    } else {
        "not routed here".to_owned()
    };
    (format!("{name}  {why}"), theme.tone(Tone::Attention), false)
}

/// The names this machine routes for the box, `.localhost, .test`; `None`
/// from daemons that do not say.
fn suffixes(host: &BoxHost) -> Option<String> {
    let names: Vec<String> = host
        .suffixes
        .iter()
        .map(|suffix| format!(".{}", clean(suffix)))
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// A sync's state in words, with its tone.
fn sync_state(state: &str) -> (&'static str, Tone) {
    match state {
        "watching" => ("up to date, following changes", Tone::Ok),
        "comparing" => ("comparing with the box", Tone::Busy),
        "syncing" => ("copying", Tone::Busy),
        "failed" => ("failed", Tone::Failure),
        "interrupted" => ("interrupted", Tone::Attention),
        _ => ("waiting", Tone::Quiet),
    }
}

/// A box lane's overview: the box, its services, tasks and infra, what is
/// mirrored and the URLs that open here.
fn lane_lines(
    app: &App,
    host: &BoxHost,
    box_lane: &str,
    lane: &LaneSummary,
    width: usize,
) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let plain = Style::new();
    let mut lines = Vec::new();
    let mut about = vec![clean(&host.host)];
    about.extend(host.transport.as_deref().map(clean));
    about.extend(host.version().map(|v| format!("devlanes {}", clean(&v))));
    lines.extend(field(theme, "box", &about.join(" · "), width, plain));
    if let Some(domain) = &lane.domain {
        lines.extend(field(theme, "domain", domain, width, plain));
    }
    let services: Vec<_> = lane
        .units
        .iter()
        .filter(|u| tree::role(u) == Role::Service)
        .collect();
    if !services.is_empty() {
        let up = services
            .iter()
            .filter(|u| matches!(u.state.as_str(), "ready" | "running"))
            .count();
        lines.extend(field(
            theme,
            "services",
            &format!("{up} of {} up", services.len()),
            width,
            plain,
        ));
        for unit in services
            .iter()
            .filter(|u| !matches!(u.state.as_str(), "ready" | "running"))
            .take(8)
        {
            let look = unit_look(theme, unit, app.spin);
            lines.push(fit(
                vec![
                    Span::raw(" ".repeat(LABEL)),
                    Span::styled(format!("{} ", look.glyph), theme.tone(look.tone)),
                    Span::raw(format!("{:<16} ", clip(&clean(unit.label()), 16))),
                    Span::styled(look.word, theme.tone(look.word_tone)),
                ],
                width,
            ));
        }
    }
    let tasks: Vec<_> = lane
        .units
        .iter()
        .filter(|u| tree::role(u) == Role::Task)
        .collect();
    if !tasks.is_empty() {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for task in &tasks {
            let word = unit_look(theme, task, 0).word;
            match counts.iter_mut().find(|(w, _)| *w == word) {
                Some(entry) => entry.1 += 1,
                None => counts.push((word, 1)),
            }
        }
        let parts: Vec<String> = counts
            .into_iter()
            .map(|(word, count)| format!("{count} {word}"))
            .collect();
        lines.extend(field(theme, "tasks", &parts.join(", "), width, plain));
    }
    let infra: Vec<String> = lane
        .containers
        .iter()
        .filter(|c| !c.hidden)
        .map(|c| {
            let port = c
                .ports
                .values()
                .next()
                .map(|p| format!(" :{p}"))
                .unwrap_or_default();
            format!("{}{port}", clean(&c.name))
        })
        .collect();
    if !infra.is_empty() {
        lines.extend(field(theme, "infra", &infra.join(", "), width, plain));
    }
    let ports: Vec<_> = host.ports.iter().filter(|p| p.lane == box_lane).collect();
    let bound = ports
        .iter()
        .filter(|p| p.state.as_deref() == Some("bound"))
        .count();
    if host.skipped(box_lane).is_none() && !ports.is_empty() {
        let mirrored = format!("{bound} of {} mirrored here", ports.len());
        lines.extend(field(theme, "ports", &mirrored, width, plain));
    }
    if let Some(proxy) = proxy_line(host) {
        lines.extend(field(theme, "proxy", &proxy, width, plain));
    }
    let urls: Vec<Entry> = host
        .routes
        .iter()
        .filter(|route| route.lane == box_lane)
        .filter_map(|route| route.url.as_deref().map(|url| (clean(url), plain, true)))
        .collect();
    lines.extend(group(theme, "urls", &urls, width));
    for (index, sync) in host.syncs.iter().filter(|s| s.lane == box_lane).enumerate() {
        let label = if index == 0 { "syncs" } else { "" };
        let (state, tone) = sync_state(&sync.state);
        let lead = format!("{}  ", clean(&sync.unit));
        lines.extend(row(theme, label, &lead, state, theme.tone(tone), width));
    }
    lines
}

/// How this machine serves the box's hostnames when the box's proxy uses
/// another HTTPS port: `proxy.https_port` is this machine's, `box_port` the
/// box's. `None` when both use the same port.
fn proxy_line(host: &BoxHost) -> Option<String> {
    let proxy = host.proxy.as_ref()?;
    let theirs = proxy.box_port?;
    let own = proxy.https_port?;
    Some(if proxy.open {
        format!("this machine serves its hostnames on {own} and on {theirs}, which its URLs use")
    } else {
        format!("this machine serves its hostnames on {own}, but its URLs use {theirs}")
    })
}

/// A section or group of a box lane: one line per unit with its state and
/// the URL that opens it here.
fn members_table(
    app: &App,
    lane: &LaneSummary,
    members: &[RowId],
    width: usize,
) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let name_width = members
        .iter()
        .filter_map(|m| match m {
            RowId::Unit(name) | RowId::Container(name) => Some(text::width(name)),
            _ => None,
        })
        .max()
        .unwrap_or(4)
        .clamp(4, 24);
    let mut lines = vec![fit(
        vec![Span::styled(
            format!("  {:<name_width$}  {:<9} url", "name", "state"),
            theme.quiet(),
        )],
        width,
    )];
    for member in members {
        let (glyph, tone, name, word, url, hidden) = match member {
            RowId::Unit(name) => {
                let Some(unit) = lane.units.iter().find(|u| u.name == *name) else {
                    continue;
                };
                let look = unit_look(theme, unit, app.spin);
                (
                    look.glyph,
                    look.tone,
                    clean(tree::short_name(unit)),
                    look.word,
                    unit.urls.first().cloned().unwrap_or_default(),
                    unit.hidden,
                )
            }
            RowId::Container(name) => {
                let Some(container) = lane.containers.iter().find(|c| c.name == *name) else {
                    continue;
                };
                let (glyph, tone, word) = container_look(app, Some(lane), container);
                (
                    glyph,
                    tone,
                    clean(name),
                    word,
                    container.urls.first().cloned().unwrap_or_default(),
                    container.hidden,
                )
            }
            _ => continue,
        };
        let name_style = if hidden { theme.quiet() } else { Style::new() };
        lines.push(fit(
            vec![
                Span::styled(format!("{glyph} "), theme.tone(tone)),
                Span::styled(
                    format!("{:<name_width$}  ", clip(&name, name_width)),
                    name_style,
                ),
                Span::styled(
                    format!("{word:<9} "),
                    theme.tone(if tone == Tone::Ok { Tone::Quiet } else { tone }),
                ),
                Span::styled(clean(&url), theme.quiet()),
            ],
            width,
        ));
    }
    lines
}

// ---- the boxes view ---------------------------------------------------------

/// The boxes view: the boxes this machine connects to on top, one block
/// each, and this machine as a box below, with the clients that connect to
/// it. One selection moves through both.
pub(super) fn render(app: &App, frame: &mut Frame, area: Rect) {
    let clients = clients_content(app, usize::from(area.width.saturating_sub(2)));
    // This machine takes what it needs, up to half of the page; the boxes
    // get the rest.
    let needs = (clients.note.len() + clients.rows.len().max(clients.empty.len())).max(1);
    let bottom = u16::try_from(needs + 2)
        .unwrap_or(u16::MAX)
        .min((area.height / 2).max(4))
        .min(area.height);
    let top = Rect::new(area.x, area.y, area.width, area.height - bottom);
    render_hosts(app, frame, top);
    let below = Rect::new(area.x, top.bottom(), area.width, bottom);
    render_clients(app, frame, below, clients);
}

/// The boxes this machine connects to, the selected one in view.
fn render_hosts(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let hosts = &app.boxes.hosts;
    app.boxes.hosts_box.set(area);
    app.boxes.host_blocks.borrow_mut().clear();
    if area.height < 3 {
        return;
    }
    let inner = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    let width = usize::from(inner.width).max(1);
    let focused = app.boxes.part == BoxPart::Hosts && !hosts.is_empty();
    let selected = app.boxes.host.min(hosts.len().saturating_sub(1));
    let mut lines = Vec::new();
    // Each box's first and last line, with its index.
    let mut blocks = Vec::new();
    if hosts.is_empty() {
        lines.extend(empty_lines(theme, width));
    }
    for (index, host) in hosts.iter().enumerate() {
        if index > 0 {
            lines.push(Line::from(""));
        }
        let start = lines.len();
        let (mut block, skipped_rows) = host_lines(app, host, width);
        // The box's first line, or every line of its selected skipped lane.
        let rows = match app.boxes.lane {
            None => Some(0..1),
            Some(lane) => skipped_rows.get(lane).cloned(),
        };
        if focused
            && index == selected
            && let Some(rows) = rows
        {
            for line in block.iter_mut().take(rows.end).skip(rows.start) {
                *line = highlight(theme, std::mem::take(line), width);
            }
        }
        lines.extend(block);
        blocks.push((start, lines.len(), index));
    }
    let height = usize::from(inner.height);
    // The fewest lines scrolled off the top that show the selected box,
    // from its first line when it is taller than the box. The view starts
    // at a box's first line, never partway through one.
    let first = match blocks.get(selected) {
        Some((start, end, _)) if lines.len() > height => {
            let least = end.saturating_sub(height).min(*start);
            blocks
                .iter()
                .map(|(start, _, _)| *start)
                .find(|start| *start >= least)
                .unwrap_or(least)
        }
        _ => 0,
    };
    let connected = hosts.iter().filter(|h| h.connected()).count();
    let mut right = Vec::new();
    if !hosts.is_empty() {
        let more = if lines.len() > height {
            " · j k for more"
        } else {
            ""
        };
        right.push(Span::styled(
            format!(" {connected} of {} connected{more} ", hosts.len()),
            theme.quiet(),
        ));
    }
    let left = vec![Span::styled(
        " boxes ",
        Style::new().add_modifier(Modifier::BOLD),
    )];
    draw_box(app, frame, area, left, right, focused);
    for (start, end, index) in blocks {
        let top = start.max(first);
        let bottom = end.min(first + height);
        if top < bottom {
            let y = inner.y + u16::try_from(top - first).unwrap_or(0);
            let rows = u16::try_from(bottom - top).unwrap_or(0);
            let rect = Rect::new(inner.x, y, inner.width, rows);
            app.boxes.host_blocks.borrow_mut().push((rect, index));
        }
    }
    let shown: Vec<Line> = lines.into_iter().skip(first).take(height).collect();
    frame.render_widget(Paragraph::new(shown), inner);
}

/// `line` in the selection's colors across `width` cells.
fn highlight(theme: &Theme, line: Line<'static>, width: usize) -> Line<'static> {
    let mut spans: Vec<Span> = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content, theme.selected(s.style)))
        .collect();
    let pad = width.saturating_sub(super::spans_width(&spans));
    spans.push(Span::styled(" ".repeat(pad), theme.selection()));
    Line::from(spans)
}

/// No box yet: the key that connects one and what a box is.
fn empty_lines(theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled("no boxes yet", Style::new().add_modifier(Modifier::BOLD)),
        Line::from(""),
    ];
    lines.extend(sentence(
        "connect to a box by its SSH name or a command",
        width,
        vec![
            Span::raw("  "),
            Span::styled("n", theme.key()),
            Span::raw("   "),
        ],
        theme.quiet(),
    ));
    lines.push(Line::from(""));
    lines.extend(sentence(
        "A box is another machine that runs devlanes. Its projects show up in the top bar. Their ports and hostnames work on this machine.",
        width,
        Vec::new(),
        theme.quiet(),
    ));
    lines
}

// ---- this machine as a box --------------------------------------------------

/// The lines of the clients box: notes on top, then one row per client, or
/// what to do while there is none.
struct ClientLines {
    note: Vec<Line<'static>>,
    rows: Vec<Line<'static>>,
    /// Shown instead of rows while no client is listed.
    empty: Vec<Line<'static>>,
    /// The counts for the box title.
    counts: Vec<Span<'static>>,
}

/// `text` wrapped in `width` cells under a one-cell margin, in `style`.
fn note(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    text::wrap(text, width.saturating_sub(1).max(1))
        .into_iter()
        .map(|part| fit(vec![Span::raw(" "), Span::styled(part, style)], width))
        .collect()
}

/// The clients box's content in `width` cells.
fn clients_content(app: &App, width: usize) -> ClientLines {
    let theme = &app.theme;
    let list = app.boxes.clients.list.as_ref();
    let off = list.and_then(|list| list.allow_connections) == Some(false);
    let mut lines = ClientLines {
        note: Vec::new(),
        rows: Vec::new(),
        empty: Vec::new(),
        counts: Vec::new(),
    };
    let attention = theme.tone(Tone::Attention);
    if off {
        let text = if list.is_some_and(|list| list.remote_invalid) {
            "Connections to this machine are off because config.yaml has invalid remote settings. Run devlanes doctor to see them."
        } else {
            "Connections to this machine are off. Press o to turn them on."
        };
        lines.note.extend(note(text, width, attention));
    } else if list.and_then(|list| list.require_pairing) == Some(false) {
        lines.note.extend(note(
            "remote.require_pairing is false, so clients connect without a code.",
            width,
            attention,
        ));
    }
    match list {
        Some(list) if !list.items.is_empty() => {
            let (rows, counts) = clients_rows(app, list, width);
            lines.rows = rows;
            lines.counts = counts;
        }
        _ if off => {}
        _ => lines.empty.extend(note(
            "No other machine connects here yet. Press p for a pairing code.",
            width,
            theme.quiet(),
        )),
    }
    lines
}

/// The clients box: its notes, then the client rows from the one that keeps
/// the selection in view, and where they are for the mouse.
fn render_clients(app: &App, frame: &mut Frame, area: Rect, content: ClientLines) {
    let theme = &app.theme;
    app.boxes.clients_box.set(area);
    app.boxes.clients_rows.set((Rect::default(), 0));
    if area.height < 3 {
        return;
    }
    let focused = app.boxes.part == BoxPart::Clients;
    let inner = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let notes = u16::try_from(content.note.len())
        .unwrap_or(u16::MAX)
        .min(inner.height);
    let rows = Rect::new(inner.x, inner.y + notes, inner.width, inner.height - notes);
    let room = usize::from(rows.height);
    let count = content.rows.len();
    let selected = app.boxes.clients.selected.min(count.saturating_sub(1));
    // The fewest rows scrolled off the top that still show the selection.
    let first = if count <= room {
        0
    } else {
        (selected + 1).saturating_sub(room).min(count - room)
    };
    let mut right = Vec::new();
    if !content.counts.is_empty() {
        right.push(Span::raw(" "));
        right.extend(content.counts);
        // Clients past the box's end would go unnoticed without it.
        if count > room && room > 0 {
            right.push(Span::styled(
                format!(" · rows {}-{}/{count}", first + 1, first + room),
                theme.quiet(),
            ));
        }
        right.push(Span::raw(" "));
    }
    let left = vec![Span::styled(
        " this machine ",
        Style::new().add_modifier(Modifier::BOLD),
    )];
    draw_box(app, frame, area, left, right, focused);
    frame.render_widget(
        Paragraph::new(content.note),
        Rect::new(inner.x, inner.y, inner.width, notes),
    );
    let shown: Vec<Line> = if count == 0 {
        content.empty
    } else {
        content
            .rows
            .into_iter()
            .enumerate()
            .skip(first)
            .take(room)
            .map(|(index, line)| {
                if focused && index == selected {
                    highlight(theme, line, usize::from(rows.width))
                } else {
                    line
                }
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(shown), rows);
    if count > 0 {
        app.boxes.clients_rows.set((rows, first));
    }
}

/// `● laptop  dl_cid_3f2a9c1e  203.0.113.4  connected for 2h 14m  2 pairings
/// mirrors 5 of 6 ports`: one aligned row per client in `width` cells, and
/// the counts for the title. Columns the width cannot hold go in this order:
/// the address, what it mirrors, its pairing, its id.
fn clients_rows(
    app: &App,
    list: &crate::clients::Clients,
    width: usize,
) -> (Vec<Line<'static>>, Vec<Span<'static>>) {
    let theme = &app.theme;
    let quiet = theme.quiet();
    let mut counts = Vec::new();
    let (connected, paired) = (list.connected(), list.paired());
    if connected > 0 {
        counts.push(Span::styled(
            format!("{connected} connected"),
            theme.tone(Tone::Ok),
        ));
    }
    if paired > 0 {
        if !counts.is_empty() {
            counts.push(Span::styled(" · ", quiet));
        }
        counts.push(Span::styled(format!("{paired} paired"), quiet));
    }
    let now = app.boxes.clients.now_ms();
    // Each client's cells: [name, id, address, seen, pairing, mirror].
    let cells: Vec<[(String, Style); 6]> = list
        .items
        .iter()
        .map(|client| client_cells(app, client, now))
        .collect();
    let widths: Vec<usize> = (0..6)
        .map(|column| {
            cells
                .iter()
                .map(|row| text::width(&row[column].0))
                .max()
                .unwrap_or(0)
        })
        .collect();
    // A blank column and the glyph take three cells, and two spaces follow
    // every cell but the last: 3 + the cells + 2 per cell - 2.
    let total = |keep: &[usize]| 1 + keep.iter().map(|c| widths[*c] + 2).sum::<usize>();
    let mut keep: Vec<usize> = (0..6).filter(|c| widths[*c] > 0).collect();
    for drop in [2, 5, 4, 1] {
        if total(&keep) <= width {
            break;
        }
        keep.retain(|c| *c != drop);
    }
    let rows = list
        .items
        .iter()
        .zip(&cells)
        .map(|(client, row)| {
            let (glyph, tone) = if client.connected() {
                (theme.glyph(Glyph::Ready), Tone::Ok)
            } else {
                (theme.glyph(Glyph::Stopped), Tone::Quiet)
            };
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(format!("{glyph} "), theme.tone(tone)),
            ];
            for (place, column) in keep.iter().enumerate() {
                let (cell, style) = &row[*column];
                // The last cell needs no padding.
                let shown = if place + 1 == keep.len() {
                    cell.clone()
                } else {
                    format!("{cell:<pad$}  ", pad = widths[*column])
                };
                spans.push(Span::styled(shown, *style));
            }
            fit(spans, width)
        })
        .collect();
    (rows, counts)
}

/// One client's cells, each with its style: name, short id, address, when
/// it connected or was last seen, its pairing and what it mirrors.
fn client_cells(app: &App, client: &Client, now: Option<u64>) -> [(String, Style); 6] {
    let theme = &app.theme;
    let quiet = theme.quiet();
    let name = clip(&clean(&client.name), 24);
    let name = if name.is_empty() {
        "unnamed".to_owned()
    } else {
        name
    };
    let address = client.address().map(clean).unwrap_or_default();
    let since = |ms: u64| now.map(|now| now.saturating_sub(ms));
    let seen = match client.connected_since() {
        Some(ms) => match since(ms) {
            Some(elapsed) => format!("connected for {}", connected_for(elapsed)),
            None => "connected".to_owned(),
        },
        None if client.connected() => "connected".to_owned(),
        None => match client.last_seen_ms.and_then(since) {
            Some(elapsed) => format!("last seen {}", ago(elapsed)),
            None => "not seen yet".to_owned(),
        },
    };
    let seen = match client.connections.len() {
        0 | 1 => seen,
        n => format!("{seen} ({n} connections)"),
    };
    let (pairing, pairing_style) = match (client.paired, client.pairings) {
        (false, _) => ("not paired".to_owned(), theme.tone(Tone::Attention)),
        (true, 0 | 1) => ("paired".to_owned(), quiet),
        (true, pairings) => (format!("{pairings} pairings"), quiet),
    };
    let mirror = client
        .mirror()
        .map(|mirror| {
            let ports = if mirror.ports == 1 { "port" } else { "ports" };
            let mut text = format!("mirrors {} of {} {ports}", mirror.bound, mirror.ports);
            match mirror.skipped_lanes {
                0 => {}
                1 => text.push_str(", 1 lane skipped"),
                n => text.push_str(&format!(", {n} lanes skipped")),
            }
            text
        })
        .unwrap_or_default();
    let seen_style = if client.connected() {
        Style::new()
    } else {
        quiet
    };
    [
        (name, Style::new().add_modifier(Modifier::BOLD)),
        (short_id(&client.id), quiet),
        (address, quiet),
        (seen, seen_style),
        (pairing, pairing_style),
        (mirror, quiet),
    ]
}

/// `40s`, `5m`, `2h 14m` or `3d 2h` for `elapsed` milliseconds: whole
/// minutes under an hour, as `devlanes daemon clients` counts them.
fn connected_for(elapsed: u64) -> String {
    let seconds = elapsed / 1000;
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{seconds}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// `just now`, `5m ago`, `3h ago` or `2d ago` for `elapsed` milliseconds,
/// as `devlanes daemon clients` words it.
fn ago(elapsed: u64) -> String {
    match elapsed / 1000 {
        0..60 => "just now".into(),
        seconds @ 60..3600 => format!("{}m ago", seconds / 60),
        seconds @ 3600..86_400 => format!("{}h ago", seconds / 3600),
        seconds => format!("{}d ago", seconds / 86_400),
    }
}

/// A box's glyph and tone by link state.
fn link_look(theme: &Theme, host: &BoxHost, spin: u64) -> (&'static str, Tone) {
    match host.state.as_str() {
        "connected" => (theme.glyph(Glyph::Ready), Tone::Ok),
        "connecting" | "reconnecting" => (theme.spinner(spin), Tone::Busy),
        "needs_auth" | "needs_pairing" => (theme.glyph(Glyph::Attention), Tone::Attention),
        "failed" => (theme.glyph(Glyph::Failed), Tone::Failure),
        _ => (theme.glyph(Glyph::Stopped), Tone::Quiet),
    }
}

/// `2026-10-26 10:02` in the TUI's time zone.
fn when(app: &App, ms: u64) -> String {
    let Ok(stamp) = jiff::Timestamp::from_millisecond(i64::try_from(ms).unwrap_or(i64::MAX)) else {
        return String::new();
    };
    stamp
        .to_zoned(app.time_zone.clone())
        .strftime("%Y-%m-%d %H:%M")
        .to_string()
}

/// One box: its name and link state, then the facts that apply. Also the
/// lines each skipped lane takes, which the selection can be on.
fn host_lines(
    app: &App,
    host: &BoxHost,
    width: usize,
) -> (Vec<Line<'static>>, Vec<std::ops::Range<usize>>) {
    let theme = &app.theme;
    let plain = Style::new();
    let quiet = theme.quiet();
    let name = clean(&host.host);
    let (glyph, tone) = link_look(theme, host, app.spin);
    let state = match host.state.as_str() {
        "needs_auth" => "needs a sign-in".to_owned(),
        "needs_pairing" => "needs pairing".to_owned(),
        "disconnected" | "stopped" => "not connected".to_owned(),
        state => clean(state),
    };
    let mut right = vec![Span::styled(state, theme.tone(tone))];
    if let Some(transport) = &host.transport {
        right.push(Span::styled(format!(" · {}", clean(transport)), quiet));
    }
    let mut lines = vec![spread(
        vec![
            Span::styled(format!("{glyph} "), theme.tone(tone)),
            Span::styled(name.clone(), Style::new().add_modifier(Modifier::BOLD)),
        ],
        right,
        width,
    )];
    let fact = |label: &str, value: &str, style: Style| -> Vec<Line<'static>> {
        field(theme, label, value, width.saturating_sub(2), style)
            .into_iter()
            .map(|line| {
                let mut spans = vec![Span::raw("  ")];
                spans.extend(line.spans);
                Line::from(spans)
            })
            .collect()
    };
    match host.state.as_str() {
        "connected" => {
            let link = match host.connected_since_ms {
                Some(ms) => format!("connected since {}", when(app, ms)),
                None => "connected".to_owned(),
            };
            lines.extend(fact("link", &link, plain));
        }
        "connecting" => lines.extend(fact("link", "connecting…", theme.tone(Tone::Busy))),
        "reconnecting" => {
            let link = crate::connect::next_try(host.retry_in_ms, app.boxes.age);
            lines.extend(fact("link", &link, theme.tone(Tone::Attention)));
        }
        "needs_auth" => {
            lines.extend(fact(
                "link",
                "waiting for you to sign in again, with ports and hostnames kept",
                theme.tone(Tone::Attention),
            ));
        }
        "needs_pairing" => {
            lines.extend(fact(
                "link",
                "waiting for a new pairing code, with ports and hostnames kept",
                theme.tone(Tone::Attention),
            ));
        }
        "failed" => lines.extend(fact(
            "link",
            "failed, so nothing is mirrored",
            theme.tone(Tone::Failure),
        )),
        _ => {
            let link = match host.last_connected_ms {
                Some(ms) => format!("not connected since {}", when(app, ms)),
                None => "not connected".to_owned(),
            };
            lines.extend(fact("link", &link, quiet));
        }
    }
    if let Some(error) = &host.error {
        let tone = if host.state == "failed" {
            Tone::Failure
        } else {
            Tone::Attention
        };
        // The daemon marks commands with backticks; this page has no code
        // style for them.
        lines.extend(fact("error", &error.replace('`', ""), theme.tone(tone)));
    }
    // What Enter on the box does, which the daemon's `fix` gives as a command.
    let fix = match host.state.as_str() {
        "connected" | "connecting" | "reconnecting" => None,
        "needs_auth" => Some("enter signs in again".to_owned()),
        "needs_pairing" => Some(format!(
            "run devlanes daemon pair on {name}, then press enter here"
        )),
        _ => Some("enter connects again".to_owned()),
    };
    if let Some(fix) = fix {
        lines.extend(fact("fix", &fix, theme.key()));
    }
    let mut about = Vec::new();
    about.extend(host.version().map(|v| format!("devlanes {}", clean(&v))));
    let info = host.info.as_ref();
    if let Some(port) = info.and_then(|i| i.https_port) {
        about.push(format!("proxy on {port}"));
    }
    if let Some(range) = info.and_then(|i| i.ports_range.as_deref()) {
        about.push(format!("ports.range {}", clean(range)));
    }
    if !about.is_empty() {
        lines.extend(fact("box", &about.join(" · "), plain));
    }
    // A port that cannot be served comes with a `proxy_port_in_use` warning below.
    if let Some(text) = proxy_line(host).filter(|_| host.proxy.as_ref().is_some_and(|p| p.open)) {
        lines.extend(fact("proxy", &text, plain));
    }
    // `.localhost` is what every machine routes; only a configured TLD is news.
    if host.suffixes.iter().any(|s| s != "localhost")
        && let Some(names) = suffixes(host)
    {
        lines.extend(fact(
            "names",
            &format!("hostnames under {names} are routed here"),
            plain,
        ));
    }
    if let Some(registry) = host.registry.as_ref().filter(|r| !r.lanes.is_empty()) {
        let bound = host
            .ports
            .iter()
            .filter(|p| p.state.as_deref() == Some("bound"))
            .count();
        let lanes = registry.lanes.len();
        let routes = host.routes.len();
        let ports = match host.ports.len() {
            0 => "no ports bound here".to_owned(),
            1 => format!("{bound} of 1 port bound here"),
            n => format!("{bound} of {n} ports bound here"),
        };
        let parts = [
            format!("{lanes} {}", if lanes == 1 { "lane" } else { "lanes" }),
            ports,
            format!(
                "{routes} {} routed",
                if routes == 1 { "hostname" } else { "hostnames" }
            ),
        ];
        lines.extend(fact("mirror", &parts.join(" · "), plain));
    }
    // Lanes `m` or `devlanes remote use` picked this box for.
    if !host.chosen_lanes.is_empty() {
        let chosen: Vec<String> = host.chosen_lanes.iter().map(|lane| clean(lane)).collect();
        lines.extend(fact("chosen", &chosen.join(", "), plain));
    }
    let mut skipped_rows = Vec::new();
    for (index, skipped) in host.skipped_lanes.iter().enumerate() {
        let start = lines.len();
        let label = if index == 0 { "skipped" } else { "" };
        let text = skipped_words(skipped)
            .unwrap_or_else(|| format!("{}: {}", clean(&skipped.lane), skip_reason(app, skipped)));
        lines.extend(fact(label, &text, theme.tone(Tone::Attention)));
        skipped_rows.push(start..lines.len());
    }
    for (index, refused) in host.skipped_hosts.iter().enumerate() {
        let label = if index == 0 && host.skipped_lanes.is_empty() {
            "skipped"
        } else {
            ""
        };
        let under = suffixes(host).map_or_else(
            || "outside the names this machine routes".to_owned(),
            |names| format!("not under {names}"),
        );
        let text = format!(
            "{} ({}:{}): {under}",
            clean(&refused.host),
            clean(&refused.lane),
            clean(&refused.unit)
        );
        lines.extend(fact(label, &text, theme.tone(Tone::Attention)));
    }
    for (index, sync) in host.syncs.iter().enumerate() {
        let label = if index == 0 { "syncs" } else { "" };
        let (state, tone) = sync_state(&sync.state);
        let text = format!("{}:{}  {state}", clean(&sync.lane), clean(&sync.unit));
        lines.extend(fact(label, &text, theme.tone(tone)));
    }
    // The daemon's warnings mark commands with backticks; this page has no
    // code style for them.
    for warning in host.warnings.iter().map(|w| w.replace('`', "")) {
        let first = vec![
            Span::raw("  "),
            Span::styled(
                format!("{} ", theme.glyph(Glyph::Attention)),
                theme.tone(Tone::Attention),
            ),
        ];
        lines.extend(sentence(
            &warning,
            width,
            first,
            theme.tone(Tone::Attention),
        ));
    }
    (lines, skipped_rows)
}
