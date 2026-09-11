//! The typed grammar: text in, a `Cmd` out. Nothing here touches the disk.

use crate::command::{Cmd, Wait};
use crate::model::Mark;
use crate::duration;
use anyhow::{bail, Result};
use regex::Regex;

/// One grammar, two doors: the keys in the app and the CLI both build a line and
/// send it here. Threads are folders, referenced by path, folder name, or title.
/// `GRAMMAR` below is the grammar, and is what `--help` and the key overlay print.
pub const GRAMMAR: &[&str] = &[
    "new <title>[: <problem>[, wait [1d]]]",
    "new <parent>/<title>[: <problem>]",
    "<thread>: <text>[, wait [1d]]",
    "<thread> wait [2h]",
    "done <thread>[: <text>]",
    "edit <thread> #3: <text>",
    "rename <thread>: <title>",
    "move <thread> under <parent> | top | up | down",
    "<thread> wait +1d | -2h            push a deadline out or pull it in",
    "delete <thread> | delete <thread> #3",
    "mark <thread> [#3] yellow | green | pink | blue | off",
    "restore <thread> | purge <thread>  out of the archive, or gone for good",
];

/// The last ", wait" that opens a trailing clause: any case, any spacing after the
/// comma. Byte scanning is safe because the word is ASCII, so a match can never
/// land inside a multi-byte character.
fn find_wait_clause(text: &str) -> Option<(usize, &str)> {
    let bytes = text.as_bytes();
    for i in (0..bytes.len().saturating_sub(3)).rev() {
        if !bytes[i..i + 4].eq_ignore_ascii_case(b"wait") {
            continue;
        }
        let after = &text[i + 4..];
        if !(after.is_empty() || after.starts_with(char::is_whitespace)) {
            continue;
        }
        let before = text[..i].trim_end();
        if before.ends_with(',') {
            return Some((before.len() - 1, after.trim()));
        }
    }
    None
}

/// A trailing ", wait [duration]" clause. Prose that happens to end in "wait ..."
/// stays prose; a mistyped duration is an error rather than a silently lost deadline.
pub fn split_wait(text: &str) -> Result<(String, Wait)> {
    let Some((at, rest)) = find_wait_clause(text) else {
        return Ok((text.trim().to_string(), Wait::None));
    };
    let head = text[..at].trim().to_string();
    if rest.is_empty() {
        return Ok((head, Wait::Open));
    }
    if rest.starts_with(['+', '-']) {
        bail!("to move a deadline use \"<thread> wait {rest}\" on its own");
    }
    match duration::parse(rest) {
        Ok(d) => Ok((head, Wait::For(d))),
        Err(e) if duration::looks_intended(rest) => Err(e),
        Err(_) => Ok((text.trim().to_string(), Wait::None)),
    }
}

/// "+1d" or "-2h": move a deadline rather than set one.
pub fn parse_shift(rest: &str) -> Result<Option<chrono::Duration>> {
    let rest = rest.trim();
    let Some(sign) = rest.chars().next().filter(|c| *c == '+' || *c == '-') else {
        return Ok(None);
    };
    let d = duration::parse(&rest[1..])?;
    Ok(Some(if sign == '-' { -d } else { d }))
}

pub fn wait_arg(rest: &str) -> Result<Wait> {
    let rest = rest.trim();
    if rest.is_empty() {
        Ok(Wait::Open)
    } else {
        Ok(Wait::For(duration::parse(rest)?))
    }
}

pub fn parse(line: &str) -> Result<Cmd> {
    let s = line.trim();

    if let Some(rest) = s.strip_prefix("new ") {
        // split on the first colon, so a "/" in the problem text is never a parent
        let (head, tail) = match rest.split_once(':') {
            Some((h, t)) => (h.trim(), Some(t.trim())),
            None => (rest.trim(), None),
        };
        if head.is_empty() {
            bail!("title missing: new <title>: <problem>");
        }
        let (parent, title) = match head.rsplit_once('/') {
            Some((p, t)) => (Some(p.trim().to_string()), t.trim().to_string()),
            None => (None, head.to_string()),
        };
        if title.is_empty() {
            bail!("title missing after \"/\"");
        }
        let (text, wait) = match tail {
            None => (None, Wait::None),
            Some("") => bail!("problem missing after \":\""),
            Some(t) => {
                let (text, wait) = split_wait(t)?;
                (Some(text), wait)
            }
        };
        return Ok(Cmd::New { parent, title, text, wait });
    }

    let re = |p: &str| Regex::new(p).expect("static pattern");

    if let Some(c) = re(r"^done\s+(.+?)(?:\s*:\s*(.*))?$").captures(s) {
        let text = c.get(2).map(|m| m.as_str().trim().to_string()).filter(|t| !t.is_empty());
        return Ok(Cmd::Done { thread: c[1].trim().into(), text });
    }
    if let Some(c) = re(r"^edit\s+(.+?)\s+#(\d+)\s*:\s*(.*)$").captures(s) {
        let text = c[3].trim().to_string();
        if text.is_empty() {
            bail!("text missing for #{}", &c[2]);
        }
        return Ok(Cmd::Edit { thread: c[1].trim().into(), index: c[2].parse()?, text });
    }
    if let Some(c) = re(r"^rename\s+(.+?)\s*:\s*(.*)$").captures(s) {
        let title = c[2].trim().to_string();
        if title.is_empty() {
            bail!("title missing after \":\"");
        }
        return Ok(Cmd::Rename { thread: c[1].trim().into(), title });
    }
    if let Some(c) = re(r"^move\s+(.+?)\s+top$").captures(s) {
        return Ok(Cmd::Move { thread: c[1].trim().into(), parent: None });
    }
    if let Some(c) = re(r"^move\s+(.+?)\s+up$").captures(s) {
        return Ok(Cmd::Reorder { thread: c[1].trim().into(), delta: -1 });
    }
    if let Some(c) = re(r"^move\s+(.+?)\s+down$").captures(s) {
        return Ok(Cmd::Reorder { thread: c[1].trim().into(), delta: 1 });
    }
    if let Some(c) = re(r"^restore\s+(.+)$").captures(s) {
        return Ok(Cmd::Restore { name: c[1].trim().into() });
    }
    if let Some(c) = re(r"^purge\s+(.+)$").captures(s) {
        return Ok(Cmd::Purge { name: c[1].trim().into() });
    }
    if let Some(c) = re(r"^move\s+(.+?)\s+under\s+(.+)$").captures(s) {
        return Ok(Cmd::Move { thread: c[1].trim().into(), parent: Some(c[2].trim().into()) });
    }
    if let Some(c) = re(r"^mark\s+(.+?)(?:\s+#(\d+))?\s+(\w+)$").captures(s) {
        let word = c[3].trim();
        let mark = match word {
            "off" | "none" | "clear" => None,
            w => Some(Mark::parse(w).ok_or_else(|| anyhow::anyhow!("no highlighter \"{w}\", try yellow green pink blue or off"))?),
        };
        let index = c.get(2).map(|m| m.as_str().parse()).transpose()?;
        return Ok(Cmd::Mark { thread: c[1].trim().into(), index, mark });
    }
    if let Some(c) = re(r"^delete\s+(.+?)(?:\s+#(\d+))?$").captures(s) {
        let index = c.get(2).map(|m| m.as_str().parse()).transpose()?;
        return Ok(Cmd::Delete { thread: c[1].trim().into(), index });
    }
    if let Some(c) = re(r"^([^:]+?)\s+wait\b(.*)$").captures(s) {
        let thread = c[1].trim().to_string();
        let rest = c[2].trim();
        if let Some(by) = parse_shift(rest)? {
            return Ok(Cmd::Snooze { thread, by });
        }
        return Ok(Cmd::Wait { thread, wait: wait_arg(rest)? });
    }
    if let Some(c) = re(r"^(.+?)\s*:\s*(.*)$").captures(s) {
        let thread = c[1].trim().to_string();
        if c[2].trim().is_empty() {
            bail!("text missing after \"{thread}:\"");
        }
        let (text, wait) = split_wait(&c[2])?;
        return Ok(Cmd::Event { thread, text, wait });
    }

    bail!("could not parse \"{s}\"")
}
