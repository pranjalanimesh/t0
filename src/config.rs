//! `~/.t0/config`: plain `key: value` lines, read once at startup. Today it holds
//! the palette; anything else worth settling per-machine belongs here too.

use crate::duration;
use crate::store;
use chrono::Duration;
use ratatui::style::Color;

const CONFIG_FILE: &str = "config";

const SAMPLE: &str = "\
# t0 config. Colours are #rrggbb; delete a line to go back to the default.
#
# domain      a thread with sub-threads inside it
# thread      a thread with none
# closed      a thread that is done
# event       an event line
# event_wait  a wait line, which says less than the others
# chat        a Claude Code chat
#
# domain: #c8a2ff
# thread: #e6e9ee
# closed: #8b93a1
# event: #e6e9ee
# event_wait: #8b93a1
# chat: #7aa7ff
#
# The box beside a thread is coloured by state, not by the text colours above:
#
# doing: #e0a458
# alarm: #ff6b6b
#
# The event dots keep their own colours, one per kind:
#
# origin: #f0857a
# action: #e0a458
# wait: #7aa7ff
# done: #6fc48b
#
# And the furniture:
#
# ink: #e6e9ee
# muted: #8b93a1
# faint: #6b7280
# cursor: #2a2f36
#
# The highlighter pens, painted behind a line you mark with m:
#
# yellow: #5c4d1e
# green: #274d33
# pink: #5c2844
# blue: #243d63
#
# How long a thread sits untouched before it goes cold and leaves the
# current view. Nothing is ever deleted. 30m / 2h / 1d / 2w.
#
# cools_after: 3w
";

pub struct Config {
    pub theme: Theme,
    /// How long a thread can sit untouched before it goes cold and leaves the
    /// current view. Nothing is deleted; it just stops being in the way.
    pub cools_after: Duration,
}

/// Every colour the tree is drawn with. The defaults are the built-in palette, so a
/// missing or half-filled config file changes nothing.
pub struct Theme {
    pub ink: Color,
    pub muted: Color,
    pub faint: Color,
    pub alarm: Color,
    pub cursor_bg: Color,
    /// A thread someone has started work on.
    pub doing: Color,
    /// The event dots, one per kind.
    pub origin: Color,
    pub action: Color,
    pub wait: Color,
    pub done: Color,
    /// The text of a row, which is chosen separately from its box.
    pub domain: Color,
    pub thread: Color,
    pub closed: Color,
    pub event: Color,
    pub event_wait: Color,
    pub chat: Color,
    /// The highlighter pens, painted behind a line you have marked.
    pub yellow: Color,
    pub green: Color,
    pub pink: Color,
    pub blue: Color,
}

impl Default for Theme {
    fn default() -> Theme {
        let ink = Color::Rgb(0xe6, 0xe9, 0xee);
        let muted = Color::Rgb(0x8b, 0x93, 0xa1);
        let wait = Color::Rgb(0x7a, 0xa7, 0xff);
        Theme {
            ink,
            muted,
            faint: Color::Rgb(0x6b, 0x72, 0x80),
            alarm: Color::Rgb(0xff, 0x6b, 0x6b),
            cursor_bg: Color::Rgb(0x2a, 0x2f, 0x36),
            doing: Color::Rgb(0xe0, 0xa4, 0x58),
            origin: Color::Rgb(0xf0, 0x85, 0x7a),
            action: Color::Rgb(0xe0, 0xa4, 0x58),
            wait,
            done: Color::Rgb(0x6f, 0xc4, 0x8b),
            // a domain reads as a plain thread until someone says otherwise
            domain: ink,
            thread: ink,
            closed: muted,
            event: ink,
            event_wait: muted,
            chat: wait,
            // dim enough that the text on top of them stays readable
            yellow: Color::Rgb(0x5c, 0x4d, 0x1e),
            green: Color::Rgb(0x27, 0x4d, 0x33),
            pink: Color::Rgb(0x5c, 0x28, 0x44),
            blue: Color::Rgb(0x24, 0x3d, 0x63),
        }
    }
}

/// "#rrggbb". Anything else is ignored rather than fatal: a typo in the config
/// should cost you one colour, not the app.
fn hex(s: &str) -> Option<Color> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// Reads the config, writing a commented sample the first time so the settings are
/// discoverable from the folder itself.
/// The file itself, which is the configuration system: you edit it, t0 follows.
pub fn path() -> std::path::PathBuf {
    store::root().join(CONFIG_FILE)
}

pub fn load() -> Config {
    let path = path();
    if !path.exists() {
        let _ = std::fs::write(&path, SAMPLE);
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut t = Theme::default();
    let mut cools_after = Duration::days(21);
    for line in text.lines() {
        // a comment opens with #; a colour only ever appears after the colon
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        if key.trim() == "cools_after" {
            if let Ok(d) = duration::parse(value) {
                cools_after = d;
            }
            continue;
        }
        let Some(c) = hex(value) else { continue };
        match key.trim() {
            "ink" => t.ink = c,
            "muted" => t.muted = c,
            "faint" => t.faint = c,
            "alarm" => t.alarm = c,
            "cursor" => t.cursor_bg = c,
            "doing" => t.doing = c,
            "origin" => t.origin = c,
            "action" => t.action = c,
            "wait" => t.wait = c,
            "done" => t.done = c,
            "domain" => t.domain = c,
            "thread" => t.thread = c,
            "closed" => t.closed = c,
            "event" => t.event = c,
            "event_wait" => t.event_wait = c,
            "chat" => t.chat = c,
            "yellow" => t.yellow = c,
            "green" => t.green = c,
            "pink" => t.pink = c,
            "blue" => t.blue = c,
            _ => {}
        }
    }
    Config { theme: t, cools_after }
}
