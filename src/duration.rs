//! Durations as people type them ("2d", "90m") and as t0 shows them back.

use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Local};

/// Ten years is more deadline than anyone needs, and keeps every span far inside
/// what a date can hold.
const MAX_MINUTES: i64 = 60 * 24 * 366 * 10;

/// "30m", "2 h", "1 day", "2 weeks". Generous on purpose: a wait clause that
/// does not parse is an error, so it must accept how people actually type.
pub fn parse(s: &str) -> Result<Duration> {
    let text = s.trim().to_lowercase();
    let cut = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    let (num, unit) = text.split_at(cut);
    let n: i64 = num.parse().map_err(|_| bad(s))?;
    let per = match unit.trim() {
        "m" | "min" | "mins" | "minute" | "minutes" => 1,
        "h" | "hr" | "hrs" | "hour" | "hours" => 60,
        "d" | "day" | "days" => 60 * 24,
        "w" | "wk" | "wks" | "week" | "weeks" => 60 * 24 * 7,
        _ => return Err(bad(s)),
    };
    let minutes = n.checked_mul(per).unwrap_or(i64::MAX);
    if minutes == 0 {
        bail!("bad duration \"{s}\", must be at least 1m");
    }
    if minutes > MAX_MINUTES {
        bail!("duration \"{s}\" is too far out, ten years is the limit");
    }
    Ok(Duration::minutes(minutes))
}

/// Does this look like someone meant a duration and mistyped it? Used to tell a
/// wait clause apart from prose that happens to end in "wait ...".
pub fn looks_intended(s: &str) -> bool {
    s.trim().starts_with(|c: char| c.is_ascii_digit())
}

fn bad(s: &str) -> anyhow::Error {
    anyhow::anyhow!("bad duration \"{s}\", use 30m / 2h / 1d / 1w")
}

/// Shortest label that says the same thing: 2d, 16h, 90m.
pub fn format(d: Duration) -> String {
    let m = d.num_minutes();
    if m % (60 * 24 * 7) == 0 {
        format!("{}w", m / (60 * 24 * 7))
    } else if m % (60 * 24) == 0 {
        format!("{}d", m / (60 * 24))
    } else if m % 60 == 0 {
        format!("{}h", m / 60)
    } else {
        format!("{m}m")
    }
}

/// Rounded, so a wait you just set for 2h reads "in 2h" and not "in 1h".
fn span(abs: Duration) -> String {
    let mins = abs.num_minutes().max(1);
    if abs < Duration::hours(1) {
        format!("{mins}m")
    } else if abs < Duration::hours(36) {
        format!("{}h", (mins as f64 / 60.0).round() as i64)
    } else {
        format!("{}d", (mins as f64 / 1440.0).round() as i64)
    }
}

/// "in 6h", "18h over", "in 3d"
pub fn due_label(due: DateTime<Local>, now: DateTime<Local>) -> String {
    let left = due - now;
    let abs = if left < Duration::zero() { -left } else { left };
    if abs < Duration::minutes(1) {
        return if left >= Duration::zero() { "due now".into() } else { "just passed".into() };
    }
    if left >= Duration::zero() {
        format!("in {}", span(abs))
    } else {
        format!("{} over", span(abs))
    }
}

/// "just now", "3h ago", "2d ago"
pub fn ago(at: DateTime<Local>, now: DateTime<Local>) -> String {
    let d = now - at;
    if d < Duration::minutes(2) {
        "just now".into()
    } else {
        format!("{} ago", span(d))
    }
}
