//! The on-disk text format: `thread.md` and chat front matter, parsed and written
//! so a hand-edited file survives a round trip.

use crate::duration;
use crate::model::{Chat, Event, Kind, Mark, Meta};
use chrono::{DateTime, Local};

/// thread.md, human-editable:
///
///     # aws
///
///     - 2026-09-05T09:10:00+05:30 t0 EBS throttling on prod db
///     - 2026-09-06T11:00:00+05:30 action mailed vijay
///     - 2026-09-06T11:00:00+05:30 wait 1d | due=2026-09-07T11:00:00+05:30 fired=2026-09-07T11:00:03+05:30
///
///     anything else is notes and is kept as written
pub struct ThreadFile {
    pub title: String,
    pub meta: Meta,
    pub events: Vec<Event>,
    pub notes: String,
}

fn parse_time(s: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Local))
}

fn stamp(t: DateTime<Local>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// Is this trailing chunk our metadata, or just a pipe someone typed in a note?
fn is_meta(s: &str) -> bool {
    !s.is_empty()
        && s.split_whitespace()
            .all(|p| matches!(p.split_once('='), Some(("due" | "fired" | "mark", v)) if !v.is_empty()))
}

/// "- <time> <kind> <text> | due=<time> fired=<time>". A line whose time, kind or
/// metadata does not parse is not an event, so nothing a person types is eaten.
fn parse_event(line: &str) -> Option<Event> {
    let rest = line.strip_prefix("- ")?;
    let (time, rest) = rest.split_once(' ')?;
    let at = parse_time(time)?;
    let (kind_word, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    let kind = Kind::parse(kind_word)?;
    let (text, meta) = match rest.rsplit_once(" | ") {
        Some((text, meta)) if is_meta(meta) => (text, meta),
        _ => (rest, ""),
    };
    let mut event = Event { kind, text: text.trim().to_string(), at, due: None, fired: None, mark: None };
    for pair in meta.split_whitespace() {
        let (key, value) = pair.split_once('=')?;
        match key {
            "due" => event.due = Some(parse_time(value)?),
            "fired" => event.fired = Some(parse_time(value)?),
            "mark" => event.mark = Mark::parse(value),
            _ => {}
        }
    }
    // a hand-written "wait 2d" with no due= gets the deadline it says it has
    if event.kind == Kind::Wait && event.due.is_none() && !event.text.is_empty() {
        if let Ok(d) = duration::parse(&event.text) {
            event.due = Some(at + d);
        }
    }
    Some(event)
}

/// "<!-- t0: order=2 from=infra/aws -->": ours, and invisible in rendered markdown.
/// Files written before the rename say "loom:", so both are read.
fn parse_meta(line: &str) -> Option<Meta> {
    let line = line.trim();
    let body = line
        .strip_prefix("<!-- t0:")
        .or_else(|| line.strip_prefix("<!-- loom:"))?
        .strip_suffix("-->")?;
    let mut meta = Meta::default();
    for pair in body.split_whitespace() {
        match pair.split_once('=') {
            Some(("order", v)) => meta.order = v.parse().ok(),
            Some(("from", v)) => meta.from = Some(v.to_string()),
            Some(("archived", v)) => meta.archived = parse_time(v),
            Some(("mark", v)) => meta.mark = Mark::parse(v),
            _ => {}
        }
    }
    Some(meta)
}

fn serialize_meta(m: &Meta) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(order) = m.order {
        parts.push(format!("order={order}"));
    }
    if let Some(from) = &m.from {
        parts.push(format!("from={from}"));
    }
    if let Some(at) = m.archived {
        parts.push(format!("archived={}", stamp(at)));
    }
    if let Some(mark) = m.mark {
        parts.push(format!("mark={}", mark.word()));
    }
    (!parts.is_empty()).then(|| format!("<!-- t0: {} -->", parts.join(" ")))
}

pub fn parse_thread(text: &str, fallback_title: &str) -> ThreadFile {
    let mut title = None;
    let mut meta = Meta::default();
    let mut events = Vec::new();
    let mut notes: Vec<&str> = Vec::new();
    let body = text.replace("\r\n", "\n");
    for line in body.lines() {
        if let Some(e) = parse_event(line) {
            events.push(e);
        } else if let Some(m) = parse_meta(line) {
            meta = m;
        } else if let Some(rest) = line.strip_prefix("# ") {
            if title.is_none() && events.is_empty() {
                title = Some(rest.trim().to_string());
            } else {
                notes.push(line);
            }
        } else {
            notes.push(line);
        }
    }
    ThreadFile {
        title: title.unwrap_or_else(|| fallback_title.to_string()),
        meta,
        events,
        notes: notes.join("\n").trim().to_string(),
    }
}

pub fn serialize_thread(t: &ThreadFile) -> String {
    let mut out = format!("# {}\n", t.title);
    if let Some(meta) = serialize_meta(&t.meta) {
        out.push_str(&format!("{meta}\n"));
    }
    out.push('\n');
    for e in &t.events {
        let meta = [
            e.due.map(|d| format!("due={}", stamp(d))),
            e.fired.map(|f| format!("fired={}", stamp(f))),
            e.mark.map(|m| format!("mark={}", m.word())),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        out.push_str(&format!("- {} {}", stamp(e.at), e.kind.word()));
        if !e.text.is_empty() {
            out.push_str(&format!(" {}", e.text));
        }
        if !meta.is_empty() {
            out.push_str(&format!(" | {meta}"));
        }
        out.push('\n');
    }
    if !t.notes.is_empty() {
        out.push('\n');
        out.push_str(&t.notes);
        out.push('\n');
    }
    out
}

/// chats/<file>.md front matter. A file we cannot read still shows up, named after itself.
pub fn parse_chat(file: &str, text: &str) -> Chat {
    let mut session = String::new();
    let mut title = String::new();
    let mut created = None;
    let body = text.replace("\r\n", "\n");
    if let Some(front) = body.strip_prefix("---\n").and_then(|s| s.split("\n---").next()) {
        for line in front.lines() {
            match line.split_once(':') {
                Some(("session", v)) => session = v.trim().to_string(),
                Some(("title", v)) => title = v.trim().to_string(),
                Some(("created", v)) => created = parse_time(v.trim()),
                _ => {}
            }
        }
    }
    if title.is_empty() {
        title = file.trim_end_matches(".md").to_string();
    }
    Chat { file: file.to_string(), session, title, created }
}

pub fn serialize_chat(c: &Chat) -> String {
    format!(
        "---\nsession: {}\ntitle: {}\ncreated: {}\n---\n\n",
        c.session,
        c.title,
        c.created.map(stamp).unwrap_or_default()
    )
}
