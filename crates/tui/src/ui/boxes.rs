//! Boxes on screen: the pane of a box lane, which shows a unit's details
//! (state, ports with their mirror state, hostnames with the URL that opens
//! here, syncs) where a local unit has its terminal, and the boxes view (`B`)
//! with every box's link and what it mirrors. Nothing here acts on a box.

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
use crate::app::App;
use crate::boxes::{BoxHost, RegistryUnit};
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
    let mut lines = notes(app, host, box_lane, width);
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
            let text = format!(
                "Read-only here. Its output stays on {}: {}",
                host.host,
                host.there(&command)
            );
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
            let text = format!(
                "Read-only here. It starts and stops with its lane on {}: {}",
                host.host,
                host.there(&command)
            );
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
/// last known state, and a lane this machine does not mirror.
fn notes(app: &App, host: &BoxHost, box_lane: &str, width: usize) -> Vec<Line<'static>> {
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
        "needs_auth" => Some(format!(
            "{name} needs a sign-in; this is its last known state. devlanes remote connect {name} resumes it."
        )),
        "reconnecting" | "connecting" => Some(format!(
            "{name} is reconnecting; this is its last known state."
        )),
        state => Some(format!(
            "{name} is {}; this is its last known state.",
            clean(state)
        )),
    };
    if let Some(down) = down {
        lines.extend(attention(&down, &[]));
    }
    if let Some(skipped) = host.skipped(box_lane) {
        let reason = skip_reason(app, skipped);
        lines.extend(attention(
            &format!("Not mirrored here: {reason}."),
            &["Its ports and hostnames are mirrored once that changes.".to_owned()],
        ));
    }
    lines
}

/// Why a box lane is skipped, naming the local lane that serves its
/// hostnames when one does: local lanes win over every box, so that lane is
/// the one to stop.
fn skip_reason(app: &App, skipped: &crate::boxes::SkippedLane) -> String {
    match local_owner(app, &skipped.hosts) {
        Some(local) if skipped.other.is_none() => {
            format!("{} on this machine serves its hostnames", clean(&local))
        }
        _ => clean(&skipped.reason),
    }
}

/// The local lane that serves one of `hosts`: a unit URL naming it, or the
/// lane's domain it falls under.
fn local_owner(app: &App, hosts: &[String]) -> Option<String> {
    let serves = |lane: &LaneSummary, host: &str| {
        let host = host.to_ascii_lowercase();
        let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
        lane.units.iter().flat_map(|u| &u.urls).any(|url| {
            url.strip_prefix("https://")
                .and_then(|rest| rest.split([':', '/']).next())
                .is_some_and(|named| named.eq_ignore_ascii_case(&host))
        }) || lane.domain.as_deref().is_some_and(under)
    };
    app.lanes
        .iter()
        .find(|lane| hosts.iter().any(|host| serves(lane, host)))
        .map(|lane| lane.reference.clone())
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
            // `nothing on the box accepted it: connection refused` reads
            // `last try: nothing on the box accepted it (connection refused)`.
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
            "yes, on the box: never mirrored",
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
            "; its hostnames still open it"
        } else {
            ""
        };
        return match (state, local) {
            ("local_lane", Some(local)) => (
                format!("kept for {}; not mirrored", clean(local)),
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
            .filter(|state| matches!(*state, "reconnecting" | "sign_in_needed"))
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
            || "not routed here (outside the names this machine routes)".to_owned(),
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
        format!("this machine serves its hostnames on {own}, and on {theirs} too as its URLs say")
    } else {
        format!("this machine serves its hostnames on {own} (the box's URLs use {theirs})")
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

/// The boxes view: one block per box, scrolling as one page.
pub(super) fn render(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let hosts = &app.boxes.hosts;
    let inner = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    let width = usize::from(inner.width).max(1);
    let mut lines = Vec::new();
    if hosts.is_empty() {
        lines.extend(empty_lines(theme, width));
    }
    for (index, host) in hosts.iter().enumerate() {
        if index > 0 {
            lines.push(Line::from(""));
        }
        lines.extend(host_lines(app, host, width));
    }
    let height = usize::from(inner.height);
    let max = lines.len().saturating_sub(height);
    app.boxes.max_scroll.set(max);
    let scroll = app.boxes.scroll.min(max);
    let connected = hosts.iter().filter(|h| h.connected()).count();
    let mut right = Vec::new();
    if !hosts.is_empty() {
        let more = if max > 0 { " · j k for more" } else { "" };
        right.push(Span::styled(
            format!(" {connected} of {} connected{more} ", hosts.len()),
            theme.quiet(),
        ));
    }
    let left = vec![Span::styled(
        " boxes ",
        Style::new().add_modifier(Modifier::BOLD),
    )];
    draw_box(app, frame, area, left, right, false);
    let shown: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();
    frame.render_widget(Paragraph::new(shown), inner);
}

/// No box yet: what connecting one does, laid out like the welcome's keys.
fn empty_lines(theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let command = "devlanes remote connect <box>";
    let mut lines = vec![
        Line::styled(
            "no boxes connected",
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
    ];
    lines.extend(sentence(
        "mirrors a box's lanes here: their ports, hostnames and synced folders",
        width,
        vec![
            Span::raw("  "),
            Span::styled(command, theme.key()),
            Span::raw("   "),
        ],
        theme.quiet(),
    ));
    lines.push(Line::from(""));
    lines.extend(sentence(
        "The box keeps running them; this view is read-only.",
        width,
        Vec::new(),
        theme.quiet(),
    ));
    lines
}

/// A box's glyph and tone by link state.
fn link_look(theme: &Theme, host: &BoxHost, spin: u64) -> (&'static str, Tone) {
    match host.state.as_str() {
        "connected" => (theme.glyph(Glyph::Ready), Tone::Ok),
        "connecting" | "reconnecting" => (theme.spinner(spin), Tone::Busy),
        "needs_auth" => (theme.glyph(Glyph::Attention), Tone::Attention),
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

/// One box: its name and link state, then the facts that apply.
fn host_lines(app: &App, host: &BoxHost, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let plain = Style::new();
    let quiet = theme.quiet();
    let name = clean(&host.host);
    let (glyph, tone) = link_look(theme, host, app.spin);
    let state = match host.state.as_str() {
        "needs_auth" => "needs a sign-in".to_owned(),
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
    let fix = || format!("devlanes remote connect {name}");
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
            let left = host
                .retry_in_ms
                .map(|ms| ms.saturating_sub(app.boxes.age * 1000).div_ceil(1000));
            let link = match left {
                Some(seconds) if seconds > 0 => format!("reconnecting, next try in {seconds}s"),
                _ => "reconnecting, trying now".to_owned(),
            };
            lines.extend(fact("link", &link, theme.tone(Tone::Attention)));
        }
        "needs_auth" => {
            lines.extend(fact(
                "link",
                "waiting for you to sign in again; ports and hostnames stay",
                theme.tone(Tone::Attention),
            ));
        }
        "failed" => lines.extend(fact(
            "link",
            "failed; nothing is mirrored",
            theme.tone(Tone::Failure),
        )),
        _ => {
            let link = match host.last_connected_ms {
                Some(ms) => format!("not connected; last connected {}", when(app, ms)),
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
        lines.extend(fact("error", error, theme.tone(tone)));
    }
    if !matches!(
        host.state.as_str(),
        "connected" | "connecting" | "reconnecting"
    ) {
        let command = host.fix.clone().unwrap_or_else(fix);
        lines.extend(fact("fix", &command, theme.key()));
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
    for (index, skipped) in host.skipped_lanes.iter().enumerate() {
        let label = if index == 0 { "skipped" } else { "" };
        let text = format!("{}: {}", clean(&skipped.lane), skip_reason(app, skipped));
        lines.extend(fact(label, &text, theme.tone(Tone::Attention)));
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
    lines
}
