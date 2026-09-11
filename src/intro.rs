//! The banner that opens the app: the wordmark resolves out of static, the tagline
//! types itself under it, and the whole thing rolls up into the header. Any key
//! cuts it short.

use crate::config::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::Duration;

const ART: &[&str] = &[
    "   ▄▄            ",
    " ▄▄██▄▄   ▄████▄ ",
    "   ██     ██  ██ ",
    "   ██     ██  ██ ",
    "   ▀██▄   ▀████▀ ",
];
const TAGLINE: &str = "the open loops in your work";

/// The glyphs a cell flickers through before it settles.
const STATIC: &[char] = &['░', '▒', '▓', '▚', '▞', '▘', '▝'];
/// How many columns wide the flicker runs ahead of the settled letters.
const BAND: usize = 5;

const RESOLVE: Duration = Duration::from_millis(900);
const HOLD: Duration = Duration::from_millis(700);
const ROLL: Duration = Duration::from_millis(300);

pub fn over(elapsed: Duration) -> bool {
    elapsed >= RESOLVE + HOLD + ROLL
}

/// Rows the banner takes at this moment: the art, a gap, and the tagline, until it
/// rolls up from the top like a blind. It never gives back the header's own row.
pub fn height(elapsed: Duration) -> u16 {
    let full = ART.len() + 2;
    let rolling = elapsed.saturating_sub(RESOLVE + HOLD);
    if rolling.is_zero() {
        return full as u16;
    }
    let left = 1.0 - rolling.as_secs_f32() / ROLL.as_secs_f32();
    ((full as f32 * left).ceil() as usize).clamp(1, full) as u16
}

/// A different glyph every frame for every cell, with no state to keep: the frame
/// number and the cell's place are enough to pick one.
fn flicker(frame: u64, row: usize, col: usize) -> char {
    let mut x = frame.wrapping_mul(0x9E37_79B9).wrapping_add((row as u64) << 32).wrapping_add(col as u64) | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATIC[(x % STATIC.len() as u64) as usize]
}

pub fn lines(elapsed: Duration, t: &Theme) -> Vec<Line<'static>> {
    let frame = elapsed.as_millis() as u64 / 33;
    let width = ART.iter().map(|r| r.chars().count()).max().unwrap_or(0);
    // the settled edge sweeps left to right, with the flicker running just ahead of it
    let progress = (elapsed.as_secs_f32() / RESOLVE.as_secs_f32()).min(1.0);
    let front = (progress * (width + BAND) as f32) as usize;
    let settled = Style::default().fg(t.ink).add_modifier(Modifier::BOLD);
    let loose = Style::default().fg(t.wait);

    let mut out: Vec<Line> = ART
        .iter()
        .enumerate()
        .map(|(row, text)| {
            let mut spans = vec![Span::raw(" ")];
            for (col, c) in text.chars().enumerate() {
                let span = if c == ' ' || col + BAND < front {
                    Span::styled(c.to_string(), settled)
                } else if col < front {
                    Span::styled(flicker(frame, row, col).to_string(), loose)
                } else {
                    Span::raw(" ")
                };
                spans.push(span);
            }
            Line::from(spans)
        })
        .collect();

    // the tagline types itself out while the letters hold
    let typing = elapsed.saturating_sub(RESOLVE).as_secs_f32() / (HOLD.as_secs_f32() * 0.6);
    let shown = ((TAGLINE.chars().count() as f32) * typing.min(1.0)) as usize;
    let tagline: String = TAGLINE.chars().take(shown).collect();
    out.push(Line::raw(""));
    out.push(Line::from(Span::styled(format!(" {tagline}"), Style::default().fg(t.muted))));

    // rolling up: the top rows go first
    let h = height(elapsed) as usize;
    out.split_off(out.len().saturating_sub(h))
}
