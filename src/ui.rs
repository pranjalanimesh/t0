//! Rendering: the ratatui screen, and the plain-text tree the CLI prints.

use crate::app::App;
use crate::config::Theme;
use crate::rows::{Row, RowKind};
use crate::grammar::GRAMMAR;
use crate::model::{plural, Kind, Mark, Status, Thread};
use chrono::{DateTime, Duration, Local};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const HINT: &str = "n new  a event  w wait  d done  c chat  v view  x archive  X delete  u undo  A T bins  ? keys";

/// Columns on screen, not bytes or characters: a CJK title is twice as wide.
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Cut to fit, marking the cut. Never splits a character.
pub fn fit(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".into();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// `fit`, then padded out to exactly `max` columns.
fn cell(s: &str, max: usize) -> String {
    let out = fit(s, max);
    format!("{out}{}", " ".repeat(max.saturating_sub(width(&out))))
}

/// The other way t0 shows a tree: indented plain text, for the CLI and the skill.
/// `cools_after` splits current from cold; `show_cold` picks which half.
pub fn status_text(
    now: DateTime<Local>,
    roots: &[&Thread],
    cools_after: Option<Duration>,
    show_cold: bool,
) -> String {
    const NAME: usize = 30;
    let is_cold = |t: &Thread| cools_after.is_some_and(|d| t.is_cold(now, d));
    fn walk(
        t: &Thread,
        depth: usize,
        now: DateTime<Local>,
        is_cold: &dyn Fn(&Thread) -> bool,
        show_cold: bool,
        out: &mut Vec<String>,
    ) {
        // cold and current threads are never listed together
        if is_cold(t) != show_cold {
            if !show_cold {
                return;
            }
            // a live parent still leads the way down to a cold thread under it
            if !t.children.iter().any(|c| any_cold(c, is_cold)) {
                return;
            }
        }
        let note = t.note(now).unwrap_or_default();
        let said = t.said().map(|e| e.label()).unwrap_or_default();
        let chats = match t.chats.len() {
            0 => String::new(),
            n => format!(" [{n} chat{}]", plural(n)),
        };
        // the indent already shows the nesting, so the column holds just this folder
        let label = cell(&format!("{}{}", "  ".repeat(depth), t.name), NAME);
        out.push(format!("{label} {:8} {:11} {said}{chats}", t.status(now).word(), note));
        for c in &t.children {
            walk(c, depth + 1, now, is_cold, show_cold, out);
        }
    }
    let mut out = Vec::new();
    for r in roots {
        walk(r, 0, now, &is_cold, show_cold, &mut out);
    }
    if out.is_empty() {
        "nothing yet".into()
    } else {
        out.join("\n")
    }
}

/// The box beside a thread: how loud the thread is.
/// Has anything in this subtree gone cold?
fn any_cold(t: &Thread, is_cold: &dyn Fn(&Thread) -> bool) -> bool {
    is_cold(t) || t.children.iter().any(|c| any_cold(c, is_cold))
}

fn status_color(t: &Theme, s: Status) -> Color {
    match s {
        Status::Yours => t.ink,
        Status::Doing => t.doing,
        Status::Waiting => t.wait,
        Status::Overdue => t.alarm,
        Status::Closed => t.faint,
    }
}

/// The dot beside an event: which kind it is.
fn kind_color(t: &Theme, k: Kind) -> Color {
    match k {
        Kind::Origin => t.origin,
        Kind::Action => t.action,
        Kind::Wait => t.wait,
        Kind::Done => t.done,
    }
}

/// The pen swiped over a line, painted behind the words.
fn mark_color(t: &Theme, m: Mark) -> Color {
    match m {
        Mark::Yellow => t.yellow,
        Mark::Green => t.green,
        Mark::Pink => t.pink,
        Mark::Blue => t.blue,
    }
}

/// The row's text, which is chosen separately from its dot. A thread with
/// sub-threads inside it is a domain and can be coloured apart from a leaf.
fn title_style(t: &Theme, r: &Row) -> Style {
    match &r.kind {
        RowKind::Thread { status: Status::Closed, .. } => {
            Style::default().fg(t.closed).add_modifier(Modifier::CROSSED_OUT)
        }
        RowKind::Thread { children, .. } => {
            let c = if *children > 0 { t.domain } else { t.thread };
            Style::default().fg(c).add_modifier(Modifier::BOLD)
        }
        RowKind::Event { kind: Kind::Wait, .. } => Style::default().fg(t.event_wait),
        RowKind::Event { .. } => Style::default().fg(t.event),
        RowKind::Chat { .. } => Style::default().fg(t.chat),
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let filtering = app.filter.is_some();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(if filtering { 1 } else { 0 }),
            Constraint::Min(1),
            Constraint::Length(if app.prompt.is_some() { 1 } else { 0 }),
            Constraint::Length(1),
        ])
        .split(f.area());

    // one build per frame: the header counts the same rows the list draws
    let rows = app.rows();
    header(f, chunks[0], app, &rows);
    if filtering {
        filter_line(f, chunks[1], app);
    }
    list(f, chunks[2], app, &rows);
    if app.prompt.is_some() {
        prompt(f, chunks[3], app);
    }
    footer(f, chunks[4], app);
    if app.help {
        help(f, &app.config.theme, f.area(), app.help_scroll);
    }
}

fn header(f: &mut Frame, area: Rect, app: &App, rows: &[Row]) {
    let t = &app.config.theme;
    let (open, overdue) = App::counts(rows);
    let mut spans = vec![Span::styled("t0", Style::default().fg(t.ink).add_modifier(Modifier::BOLD)), Span::raw("  ")];
    if let Some(bin) = app.bin {
        spans.push(Span::styled(bin.word(), Style::default().fg(t.action).add_modifier(Modifier::BOLD)));
        let n = app.binned.len();
        spans.push(Span::styled(format!("  {n} thread{}", plural(n)), Style::default().fg(t.muted)));
    } else {
        if app.view != crate::model::View::Current {
            spans.push(Span::styled(app.view.name(), Style::default().fg(t.wait).add_modifier(Modifier::BOLD)));
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(format!("{open} open"), Style::default().fg(t.muted)));
        if overdue > 0 {
            spans.push(Span::styled(format!("  {overdue} overdue"), Style::default().fg(t.alarm)));
        }
    }
    let path = crate::store::root().to_string_lossy().to_string();
    let home = std::env::var("HOME").unwrap_or_default();
    let shown = if home.is_empty() { path } else { path.replacen(&home, "~", 1) };
    let used: usize = spans.iter().map(|s| width(&s.content)).sum();
    let room = (area.width as usize).saturating_sub(used + 2);
    if room >= 8 {
        // the root path is the least important thing here: it goes first when space runs out
        let shown = fit(&shown, room);
        spans.push(Span::raw(" ".repeat(room - width(&shown) + 1)));
        spans.push(Span::styled(shown, Style::default().fg(t.faint)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn filter_line(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.config.theme;
    let Some(input) = &app.filter else { return };
    let spans = vec![
        Span::styled("/", Style::default().fg(t.faint)),
        Span::raw(" "),
        Span::styled(input.text.clone(), Style::default().fg(t.ink)),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    f.set_cursor_position((area.x + 2 + input.cursor as u16, area.y));
}

fn list(f: &mut Frame, area: Rect, app: &App, rows: &[Row]) {
    let t = &app.config.theme;
    if rows.is_empty() {
        let text = if app.filter.is_some() { "Nothing matches." } else { "No threads yet. Press n to start one." };
        let p = Paragraph::new(Line::from(Span::styled(text, Style::default().fg(t.muted))));
        f.render_widget(p, Rect { x: area.x + 2, y: area.y + 1, width: area.width.saturating_sub(2), height: 1 });
        return;
    }
    let height = area.height as usize;
    let i = app.index(rows);
    let start = i.saturating_sub(height.saturating_sub(1) / 2).min(rows.len().saturating_sub(height.max(1)));
    let lines: Vec<Line> = rows
        .iter()
        .skip(start)
        .take(height)
        .enumerate()
        .map(|(n, r)| row_line(&app.config.theme, r, start + n == i, area.width as usize))
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn row_line(t: &Theme, r: &Row, is_cursor: bool, total: usize) -> Line<'static> {
    let indent = "  ".repeat(r.depth);
    let mut head: Vec<Span> = vec![Span::raw(format!(" {indent}"))];
    let mut tail: Vec<Span> = Vec::new();
    // the highlighter covers the words, not the margin, so the cursor's own tint on
    // the rest of the row still shows through
    let mut title_style = title_style(t, r);
    if let Some(m) = r.mark {
        title_style = title_style.bg(mark_color(t, m));
    }
    let mut note: Option<Span> = None;

    match &r.kind {
        RowKind::Thread { status, note: n, note_alarm, chats, open, deep, children } => {
            head.push(Span::styled(if !deep { "  " } else if *open { "▾ " } else { "▸ " }, Style::default().fg(t.faint)));
            head.push(Span::styled(status.glyph(), Style::default().fg(status_color(t, *status))));
            if let Some(n) = n {
                // only this thread's own missed deadline is alarming; a child's is the child's
                let color = if *note_alarm { t.alarm } else { t.muted };
                note = Some(Span::styled(format!("  {n}"), Style::default().fg(color)));
            }
            if !open {
                if *children > 0 {
                    tail.push(Span::styled(format!("{children} inside  "), Style::default().fg(t.faint)));
                }
                if *chats > 0 {
                    tail.push(Span::styled(format!("{chats} chat  "), Style::default().fg(t.wait)));
                }
            }
        }
        RowKind::Event { index, kind, when } => {
            head.push(Span::raw("  "));
            head.push(Span::styled("· ", Style::default().fg(kind_color(t, *kind))));
            tail.push(Span::styled(format!("{when}  "), Style::default().fg(t.faint)));
            tail.push(Span::styled(format!("#{index} "), Style::default().fg(t.faint)));
        }
        RowKind::Chat { when, .. } => {
            head.push(Span::raw("  "));
            head.push(Span::styled("› ", Style::default().fg(t.wait)));
            tail.push(Span::styled(format!("{when}  "), Style::default().fg(t.faint)));
            tail.push(Span::styled("chat ", Style::default().fg(t.faint)));
        }
    }

    // the time and the badges are the load-bearing parts, so the title gives way first
    let fixed: usize = head.iter().chain(tail.iter()).map(|s| width(&s.content)).sum::<usize>()
        + note.as_ref().map(|s| width(&s.content)).unwrap_or(0);
    let title = fit(&r.title, total.saturating_sub(fixed + 1));

    let mut spans = head;
    let used = fixed + width(&title);
    spans.push(Span::styled(title, title_style));
    if let Some(note) = note {
        spans.push(note);
    }
    spans.push(Span::raw(" ".repeat(total.saturating_sub(used))));
    spans.extend(tail);
    let line = Line::from(spans);
    if is_cursor {
        line.style(Style::default().bg(t.cursor_bg))
    } else {
        line
    }
}

fn prompt(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.config.theme;
    let Some(p) = &app.prompt else { return };
    let label = fit(&format!(" {} ", p.label), (area.width as usize).saturating_sub(12));
    let gutter = width(&label) + 1;
    let room = (area.width as usize).saturating_sub(gutter + 1);

    // scroll the field so the insertion point is always on screen
    let chars: Vec<char> = p.input.text.chars().collect();
    let start = p.input.cursor.saturating_sub(room);
    let shown: String = chars.iter().skip(start).take(room).collect();

    let mut spans = vec![
        Span::styled(label, Style::default().fg(Color::Black).bg(t.wait).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
    ];
    if p.input.text.is_empty() && !p.hint.is_empty() {
        spans.push(Span::styled(fit(&p.hint, room), Style::default().fg(t.faint)));
    } else {
        spans.push(Span::styled(shown, Style::default().fg(t.ink)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    f.set_cursor_position((area.x + gutter as u16 + (p.input.cursor - start) as u16, area.y));
}

fn footer(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.config.theme;
    let (text, color) = match &app.note {
        Some((true, text)) => (text.clone(), t.done),
        Some((false, text)) => (text.clone(), t.origin),
        None => (HINT.to_string(), t.faint),
    };
    let text = fit(&text, (area.width as usize).saturating_sub(2));
    f.render_widget(Paragraph::new(Line::from(Span::styled(format!(" {text}"), Style::default().fg(color)))), area);
}

const KEYS: &[(&str, &str)] = &[
    ("j k  ↓ ↑", "move"),
    ("l h  → ←", "open or close, step in and out"),
    ("space", "toggle open"),
    ("enter", "edit the title or event, or resume a chat"),
    ("n / N", "new thread beside this one / inside it"),
    ("a", "add an event, end with \", wait 1d\" to start waiting"),
    ("w", "wait for a duration, or with no deadline"),
    ("d", "mark done"),
    ("c", "new Claude Code chat in this thread's folder"),
    ("tab / shift+tab", "move under the thread above / out one level"),
    ("J / K", "move down / up among its siblings (also shift+↓ ↑)"),
    ("v / V", "next / previous view: current, open, my move, doing, waiting, overdue, closed, cold"),
    ("x", "archive a thread; on an event or a chat, delete it. u takes it back"),
    ("X", "delete a thread into the trash; asks you to type delete"),
    ("u", "undo the last command, then the one before"),
    ("A / T", "the archive / the trash: enter or r restores, X purges, same key comes back"),
    ("m", "highlighter: swipe again for the next pen, once more to take it off"),
    (",", "edit the config in $EDITOR; colours apply as soon as you save"),
    ("/", "filter"),
    (":", "type a command"),
    ("g / G", "first / last"),
    ("q", "quit"),
];

fn help_lines(t: &Theme) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = KEYS
        .iter()
        .map(|(k, what)| {
            Line::from(vec![
                Span::styled(format!(" {k:<17}"), Style::default().fg(t.muted)),
                Span::styled((*what).to_string(), Style::default().fg(t.ink)),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(" commands, on : and in the CLI", Style::default().fg(t.muted))));
    lines.push(Line::from(Span::styled("   deadlines: 30m 2h 1d 2w, or +2h / -30m to move one", Style::default().fg(t.faint))));
    for g in GRAMMAR {
        lines.push(Line::from(Span::styled(format!("   {g}"), Style::default().fg(t.ink))));
    }
    lines
}

fn help(f: &mut Frame, t: &Theme, area: Rect, scroll: usize) {
    let lines = help_lines(t);
    let widest = lines.iter().map(Line::width).max().unwrap_or(40);
    let w = (widest as u16 + 3).min(area.width.saturating_sub(2));
    let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    let visible = h.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = scroll.min(max_scroll);
    let title = if max_scroll > 0 { format!(" keys · {}/{} · j k ", scroll + visible, lines.len()) } else { " keys ".into() };
    let shown: Vec<Line> = lines.into_iter().skip(scroll).take(visible).collect();
    f.render_widget(Clear, rect);
    let block = Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(t.faint));
    f.render_widget(Paragraph::new(shown).block(block), rect);
}
