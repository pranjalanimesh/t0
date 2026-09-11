//! Claude Code sessions started from a thread, one markdown file each.

use crate::format::serialize_chat;
use crate::model::{Chat, Thread};
use crate::store::{self, CHATS_DIR};
use anyhow::{bail, Result};
use chrono::{DateTime, Local};
use std::process::Command;

/// Writes chats/<stamp>-<slug>.md with a fresh session id. The chat itself is
/// started by the caller, which owns the terminal.
pub fn create(path: &str, title: &str, now: DateTime<Local>) -> Result<Chat> {
    let dir = store::dir_of(path);
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = if title.is_empty() { "chat".to_string() } else { title };
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let name = format!("{stamp}-{}", store::slug(&title));
    let chats = dir.join(CHATS_DIR);
    std::fs::create_dir_all(&chats)?;
    let file = store::free_named(&chats, &name, ".md");
    let chat = Chat { file: file.clone(), session: uuid::Uuid::new_v4().to_string(), title, created: Some(now) };
    std::fs::write(chats.join(&file), serialize_chat(&chat))?;
    Ok(chat)
}

/// A chat in this thread that can actually be resumed. A file with no session id
/// was hand-written or half-created, and `claude` has nothing to reopen.
pub fn find<'a>(t: &'a Thread, file: &str) -> Result<&'a Chat> {
    let Some(c) = t.chats.iter().find(|c| c.file == file) else {
        bail!("no chat \"{file}\" in {}", t.path);
    };
    if c.session.is_empty() {
        bail!("{} has no session id, open it in an editor", c.file);
    }
    Ok(c)
}

/// `claude` in the thread's folder, sharing this terminal.
pub fn command(path: &str, args: &[String]) -> Command {
    let mut cmd = Command::new("claude");
    cmd.current_dir(store::dir_of(path)).args(args);
    cmd
}

pub fn start_args(session: &str) -> Vec<String> {
    vec!["--session-id".into(), session.into()]
}

pub fn resume_args(session: &str) -> Vec<String> {
    vec!["--resume".into(), session.into()]
}
