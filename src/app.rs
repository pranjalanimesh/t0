//! What the app knows: the tree it has read, where the cursor is, and how a command
//! changes both. Rows live in `rows`, keys in `keys`.

use crate::chat;
use crate::command::{self, Cmd, Effect};
use crate::config::{self, Config};
use crate::input::Input;
use crate::keys::{Action, PromptKind};
use crate::model::{find, parent_path, Thread, View};
use crate::reference::Here;
use crate::store;
use chrono::{DateTime, Local};
use std::collections::HashSet;

pub struct Prompt {
    pub label: String,
    pub hint: String,
    pub input: Input,
    pub kind: PromptKind,
}

pub struct App {
    pub tree: Vec<Thread>,
    /// Colours, read once at startup.
    pub config: Config,
    /// What is in the archive, read only while you are looking at it.
    pub archive: Vec<Thread>,
    pub in_archive: bool,
    pub view: View,
    pub now: DateTime<Local>,
    pub expanded: HashSet<String>,
    pub cursor: Option<String>,
    pub filter: Option<Input>,
    pub prompt: Option<Prompt>,
    pub help: bool,
    pub help_scroll: usize,
    pub note: Option<(bool, String)>,
    pub armed: Option<String>,
    fallback: Option<String>,
    signature: u64,
}

const STATE_FILE: &str = ".state";

impl App {
    pub fn new() -> anyhow::Result<App> {
        store::ensure_root()?;
        let mut app = App {
            tree: store::read_tree()?,
            config: config::load(),
            archive: Vec::new(),
            in_archive: false,
            view: View::Current,
            now: Local::now(),
            expanded: HashSet::new(),
            cursor: None,
            filter: None,
            prompt: None,
            help: false,
            help_scroll: 0,
            note: None,
            armed: None,
            fallback: None,
            signature: store::signature(),
        };
        app.load_state();
        if app.cursor.is_none() {
            app.cursor = app.rows().first().map(|r| r.key.clone());
        }
        Ok(app)
    }

    fn state_path() -> std::path::PathBuf {
        store::root().join(STATE_FILE)
    }

    fn load_state(&mut self) {
        let Ok(text) = std::fs::read_to_string(App::state_path()) else { return };
        for line in text.lines() {
            match line.split_once(' ') {
                Some(("open", p)) => {
                    self.expanded.insert(p.to_string());
                }
                Some(("cursor", p)) => self.cursor = Some(p.to_string()),
                _ => {}
            }
        }
    }

    pub fn save_state(&self) {
        let mut out: Vec<String> = self.expanded.iter().map(|p| format!("open {p}")).collect();
        out.sort();
        if let Some(c) = &self.cursor {
            out.push(format!("cursor {c}"));
        }
        let _ = std::fs::write(App::state_path(), out.join("\n"));
    }

    /// Pick up edits made by a chat, an editor, or the CLI.
    pub fn poll_disk(&mut self) {
        if store::signature() != self.signature {
            self.reload();
        }
    }

    fn reload(&mut self) {
        // the config lives in the store, so editing it lands here like any other change
        self.config = config::load();
        if let Ok(tree) = store::read_tree() {
            self.tree = tree;
        }
        if self.in_archive {
            self.archive = store::read_archive();
        }
        // after the read, so a change made while reading is not missed
        self.signature = store::signature();
        let keys: HashSet<String> = self.rows().into_iter().map(|r| r.key).collect();
        if self.cursor.as_ref().is_some_and(|c| keys.contains(c)) {
            return;
        }
        // the row under the cursor is gone: fall back to where it was, then upward
        let mut candidates: Vec<String> = Vec::new();
        if let Some(f) = &self.fallback {
            candidates.push(f.clone());
        }
        if let Some(c) = &self.cursor {
            let path = c.split(['#', '@']).next().unwrap_or_default().to_string();
            candidates.push(path.clone());
            let mut p = parent_path(&path).to_string();
            while !p.is_empty() {
                candidates.push(p.clone());
                p = parent_path(&p).to_string();
            }
        }
        self.cursor = candidates
            .into_iter()
            .find(|c| keys.contains(c))
            .or_else(|| self.top_row());
    }

    pub(crate) fn say(&mut self, ok: bool, text: impl Into<String>) {
        self.note = Some((ok, text.into()));
    }

    /// Run a command and fold what it did back into the view.
    pub(crate) fn run_cmd(&mut self, cmd: Cmd) -> Action {
        self.remember_fallback();
        let outcome = match command::apply_cmd(cmd, Local::now(), &Here::default()) {
            Ok(outcome) => outcome,
            Err(e) => {
                self.say(false, format!("{e:#}"));
                return Action::None;
            }
        };
        let action = match outcome.effect {
            Effect::None => Action::None,
            // a fresh thread is where you want to be next
            Effect::Created(path) => {
                self.reveal(&path);
                Action::None
            }
            Effect::Moved(to) => {
                self.remap(&to);
                Action::None
            }
            Effect::Chat { path, file, session, title } => {
                self.expanded.insert(path.clone());
                self.cursor = Some(format!("{path}@{file}"));
                Action::RunChat { path, args: chat::start_args(&session), label: title, created: Some(file) }
            }
        };
        self.say(true, outcome.message);
        self.reload();
        action
    }

    /// Put the cursor on a row, opening whatever it sits inside.
    fn reveal(&mut self, key: &str) {
        let parent = parent_path(key.split(['#', '@']).next().unwrap_or_default()).to_string();
        if !parent.is_empty() {
            self.expanded.insert(parent);
        }
        self.cursor = Some(key.to_string());
    }

    /// After a move the folder path changed: follow it, and take its open state along.
    fn remap(&mut self, to: &str) {
        let from = self.cursor.clone().unwrap_or_default();
        let moved: Vec<String> = self.expanded.iter().filter(|p| p.starts_with(&from)).cloned().collect();
        for p in moved {
            let rest = p[from.len()..].to_string();
            self.expanded.remove(&p);
            self.expanded.insert(format!("{to}{rest}"));
        }
        if let Some(parent) = Some(parent_path(to)).filter(|p| !p.is_empty()) {
            self.expanded.insert(parent.to_string());
        }
        self.cursor = Some(to.to_string());
    }

    fn remember_fallback(&mut self) {
        let rows = self.rows();
        let i = self.index(&rows);
        self.fallback = i.checked_sub(1).and_then(|j| rows.get(j)).map(|r| r.key.clone());
    }

    pub(crate) fn thread_at(&self, path: &str) -> Option<&Thread> {
        find(self.source(), path)
    }
}
