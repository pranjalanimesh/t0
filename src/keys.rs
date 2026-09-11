//! What each keypress does. Everything here ends in either a `Cmd` handed to
//! `command`, a prompt opened, or the cursor moved.

use crate::app::{App, Prompt};
use crate::command::{Cmd, Wait};
use crate::grammar;
use crate::input::Input;
use crate::model::{parent_path, plural, Mark, Thread};
use crate::rows::{Row, RowKind};
use crate::store::{self, Bin};
use crate::chat;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a prompt is collecting, and so what command its answer becomes.
pub enum PromptKind {
    Line,
    New { parent: Option<String> },
    Event { path: String },
    Wait { path: String },
    Done { path: String },
    Rename { path: String },
    EditEvent { path: String, index: usize },
    Chat { path: String },
    /// A command that only goes through once `word` has been typed out.
    Confirm { word: &'static str, cmd: Cmd },
}

/// What the main loop must do after a key, when the app cannot do it itself.
pub enum Action {
    None,
    Quit,
    /// `created` names the chat file this action just wrote, so a launch that never
    /// happens can take it back out again.
    RunChat { path: String, args: Vec<String>, label: String, created: Option<String> },
    /// Open the config in $EDITOR. It reloads by itself when the file changes.
    EditConfig,
}

impl App {
    fn ask(&mut self, label: impl Into<String>, hint: impl Into<String>, initial: &str, kind: PromptKind) {
        self.prompt = Some(Prompt { label: label.into(), hint: hint.into(), input: Input::new(initial), kind });
    }

    pub fn key(&mut self, k: KeyEvent) -> Action {
        if self.help {
            match k.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                    self.help = false;
                    self.help_scroll = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => self.help_scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => self.help_scroll = self.help_scroll.saturating_sub(1),
                _ => {}
            }
            return Action::None;
        }
        if self.prompt.is_some() {
            return self.prompt_key(k);
        }
        if self.filter.is_some() && self.filter_key(k) {
            return Action::None;
        }
        self.tree_key(k)
    }

    /// ctrl+c backs out of whichever field is open.
    fn cancels(k: KeyEvent) -> bool {
        k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL)
    }

    fn filter_key(&mut self, k: KeyEvent) -> bool {
        if self.filter.is_none() || matches!(k.code, KeyCode::Enter | KeyCode::Down | KeyCode::Up) {
            return false;
        }
        if k.code == KeyCode::Esc {
            self.filter = None;
            self.say(true, "filter cleared");
        } else if App::cancels(k) {
            self.filter = None;
        } else if !self.filter.as_mut().is_some_and(|f| f.edit(k)) {
            return false;
        }
        // keep the cursor on something visible while the list narrows
        if !self.rows().iter().any(|r| Some(&r.key) == self.cursor.as_ref()) {
            self.cursor = self.top_row();
        }
        true
    }

    fn prompt_key(&mut self, k: KeyEvent) -> Action {
        if self.prompt.is_none() {
            return Action::None;
        }
        match k.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Enter => return self.submit(),
            _ if App::cancels(k) => self.prompt = None,
            _ => {
                self.prompt.as_mut().map(|p| p.input.edit(k));
            }
        }
        Action::None
    }

    /// Prompts build a command straight from what was typed, so a colon or a slash
    /// in a title is just text and never re-parsed as grammar.
    fn build(&self, kind: &PromptKind, value: &str) -> anyhow::Result<Cmd> {
        Ok(match kind {
            PromptKind::Line => grammar::parse(value)?,
            PromptKind::New { parent } => {
                let (head, tail) = match value.split_once(':') {
                    Some((h, t)) => (h.trim(), Some(t.trim())),
                    None => (value.trim(), None),
                };
                let (text, wait) = match tail {
                    None => (None, Wait::None),
                    Some(t) => {
                        let (text, wait) = grammar::split_wait(t)?;
                        (Some(text).filter(|t| !t.is_empty()), wait)
                    }
                };
                Cmd::New { parent: parent.clone(), title: head.to_string(), text, wait }
            }
            PromptKind::Event { path } => {
                let (text, wait) = grammar::split_wait(value)?;
                Cmd::Event { thread: path.clone(), text, wait }
            }
            PromptKind::Wait { path } => match grammar::parse_shift(value)? {
                Some(by) => Cmd::Snooze { thread: path.clone(), by },
                None => Cmd::Wait { thread: path.clone(), wait: grammar::wait_arg(value)? },
            },
            PromptKind::Done { path } => {
                Cmd::Done { thread: path.clone(), text: Some(value.to_string()).filter(|v| !v.is_empty()) }
            }
            PromptKind::Rename { path } => Cmd::Rename { thread: path.clone(), title: value.to_string() },
            PromptKind::EditEvent { path, index } => {
                Cmd::Edit { thread: path.clone(), index: *index, text: value.to_string() }
            }
            PromptKind::Chat { path } => Cmd::Chat { thread: path.clone(), title: value.to_string() },
            PromptKind::Confirm { word, cmd } => {
                if value != *word {
                    anyhow::bail!("type {word} to go through with it, esc to keep it");
                }
                cmd.clone()
            }
        })
    }

    fn submit(&mut self) -> Action {
        let Some(p) = self.prompt.take() else { return Action::None };
        let value = p.input.text.trim().to_string();

        // an empty line means "no deadline" for wait, "just close it" for done,
        // and an unnamed chat
        let optional = matches!(
            p.kind,
            PromptKind::Done { .. } | PromptKind::Wait { .. } | PromptKind::Chat { .. }
        );
        if value.is_empty() && !optional {
            return Action::None;
        }

        let action = match self.build(&p.kind, &value) {
            Ok(cmd) => self.run_cmd(cmd),
            Err(e) => {
                self.say(false, format!("{e:#}"));
                Action::None
            }
        };
        // a command that failed keeps what you typed, so you can fix it in place
        if matches!(self.note, Some((false, _))) {
            self.prompt = Some(p);
        }
        action
    }

    fn tree_key(&mut self, k: KeyEvent) -> Action {
        let rows = self.rows();
        let i = self.index(&rows);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

        let move_to = |app: &mut App, j: usize| {
            if let Some(r) = rows.get(j.min(rows.len().saturating_sub(1))) {
                app.cursor = Some(r.key.clone());
            }
        };

        match k.code {
            KeyCode::Char('q') if !ctrl => return Action::Quit,
            KeyCode::Char('c') if ctrl => return Action::Quit,
            KeyCode::Char('k') if ctrl => return Action::None,
            KeyCode::Up | KeyCode::Down if k.modifiers.contains(KeyModifiers::SHIFT) => {
                self.shuffle(&rows, i, if k.code == KeyCode::Up { -1 } else { 1 });
            }
            KeyCode::Down | KeyCode::Char('j') => move_to(self, i + 1),
            KeyCode::Up | KeyCode::Char('k') => move_to(self, i.saturating_sub(1)),
            KeyCode::PageDown => move_to(self, i + 10),
            KeyCode::PageUp => move_to(self, i.saturating_sub(10)),
            KeyCode::Home | KeyCode::Char('g') => move_to(self, 0),
            KeyCode::End | KeyCode::Char('G') => move_to(self, rows.len().saturating_sub(1)),
            KeyCode::Char('?') => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::Char(c @ ('v' | 'V')) => {
                self.view = if c == 'V' { self.view.prev() } else { self.view.next() };
                self.say(true, format!("view: {}", self.view.name()));
            }
            KeyCode::Char('A') => self.look_in(Bin::Archive, 'A'),
            KeyCode::Char('T') => self.look_in(Bin::Trash, 'T'),
            KeyCode::Char('u') => self.undo(),
            KeyCode::Char('K') => self.shuffle(&rows, i, -1),
            KeyCode::Char('J') => self.shuffle(&rows, i, 1),
            KeyCode::Char('/') => {
                self.filter = Some(Input::default());
                self.say(true, "type to filter, esc to clear");
            }
            KeyCode::Esc => {
                if self.filter.take().is_some() {
                    self.say(true, "filter cleared");
                } else {
                    self.note = None;
                }
            }
            KeyCode::Char(',') => return Action::EditConfig,
            KeyCode::Char(':') => self.ask(">", "aws: mailed vijay, wait 1d", "", PromptKind::Line),
            KeyCode::Right | KeyCode::Char('l') => match rows.get(i) {
                Some(Row { kind: RowKind::Thread { open: false, deep: true, .. }, path, .. }) => {
                    self.expanded.insert(path.clone());
                }
                _ => move_to(self, i + 1),
            },
            KeyCode::Left | KeyCode::Char('h') => match rows.get(i) {
                Some(Row { kind: RowKind::Thread { open: true, .. }, path, .. }) => {
                    self.expanded.remove(path);
                }
                Some(r) => {
                    let up = if matches!(r.kind, RowKind::Thread { .. }) { parent_path(&r.path) } else { &r.path };
                    if !up.is_empty() {
                        self.cursor = Some(up.to_string());
                    }
                }
                None => {}
            },
            KeyCode::Char(' ') => {
                if let Some(Row { kind: RowKind::Thread { .. }, path, .. }) = rows.get(i) {
                    if !self.expanded.remove(path) {
                        self.expanded.insert(path.clone());
                    }
                }
            }
            KeyCode::Char('n') => {
                let parent = rows.get(i).map(|r| parent_path(&r.path).to_string()).filter(|p| !p.is_empty());
                let label = match &parent {
                    Some(p) => format!("new in {}", self.title_of(p)),
                    None => "new thread".into(),
                };
                self.ask(label, "title: problem", "", PromptKind::New { parent });
            }
            // every one of these asks a question about the row under the cursor
            KeyCode::Char(c @ ('N' | 'a' | 'w' | 'd' | 'c')) => {
                let Some(path) = rows.get(i).map(|r| r.path.clone()) else { return Action::None };
                let title = self.title_of(&path);
                let (label, hint, kind) = match c {
                    'N' => (format!("new in {title}"), "title: problem", PromptKind::New { parent: Some(path.clone()) }),
                    'a' => (title, "what happened, wait 1d", PromptKind::Event { path: path.clone() }),
                    'w' => (
                        format!("{title} · wait"),
                        "1d · +2h to push out · -30m to pull in · empty for no deadline",
                        PromptKind::Wait { path: path.clone() },
                    ),
                    'd' => (format!("{title} · done"), "how it ended, or empty", PromptKind::Done { path: path.clone() }),
                    _ => (format!("{title} · new chat"), "what this chat is about", PromptKind::Chat { path: path.clone() }),
                };
                // adding to a thread should show what you just added
                if matches!(c, 'N' | 'a') {
                    self.expanded.insert(path);
                }
                self.ask(label, hint, "", kind);
            }
            KeyCode::Char('r') if self.bin.is_some() => return self.restore(rows.get(i)),
            // swipe the highlighter again for the next pen, once more to take it off
            KeyCode::Char('m') => {
                let Some(r) = rows.get(i) else { return Action::None };
                if matches!(r.kind, RowKind::Chat { .. }) {
                    self.say(false, "a chat has no line of its own to highlight");
                    return Action::None;
                }
                let cmd =
                    Cmd::Mark { thread: r.path.clone(), index: self.event_index(r), mark: Mark::cycle(r.mark) };
                return self.run_cmd(cmd);
            }
            KeyCode::Enter => return self.enter(rows.get(i)),
            KeyCode::Char('x') => return self.remove(rows.get(i)),
            KeyCode::Char('X') => self.destroy(rows.get(i)),
            KeyCode::Tab => self.indent(&rows, i),
            KeyCode::BackTab => self.outdent(&rows, i),
            _ => {}
        }
        Action::None
    }

    /// A and T open a bin, and the same key closes it again.
    fn look_in(&mut self, bin: Bin, key: char) {
        if self.bin == Some(bin) {
            self.bin = None;
            self.say(true, "back to the tree");
        } else {
            self.bin = Some(bin);
            self.binned = store::read_bin(bin);
            self.say(true, format!("{} · enter restores · X purges · {key} back", bin.word()));
        }
        self.cursor = self.top_row();
    }

    /// Move a thread past the sibling above or below it.
    fn shuffle(&mut self, rows: &[Row], i: usize, delta: i32) {
        let Some(row) = rows.get(i) else { return };
        if let Some(bin) = self.bin {
            self.say(false, format!("the {} is ordered by when things landed in it", bin.word()));
            return;
        }
        if !matches!(row.kind, RowKind::Thread { .. }) {
            self.say(false, "only threads can be moved");
            return;
        }
        self.run_cmd(Cmd::Reorder { thread: row.path.clone(), delta });
    }

    /// Binned threads go back whole: their pieces are not separately restorable.
    fn restore(&mut self, row: Option<&Row>) -> Action {
        let Some(row) = row else { return Action::None };
        if row.depth > 0 {
            self.say(false, "restore the whole thread, not a piece of it");
            return Action::None;
        }
        self.run_cmd(Cmd::Restore { bin: self.bin, name: row.path.clone() })
    }

    fn title_of(&self, path: &str) -> String {
        self.thread_at(path).map(|t| t.title.clone()).unwrap_or_else(|| path.to_string())
    }

    fn enter(&mut self, row: Option<&Row>) -> Action {
        let Some(row) = row else { return Action::None };
        if self.bin.is_some() {
            return self.restore(Some(row));
        }
        let (path, title) = (row.path.clone(), row.title.clone());
        match &row.kind {
            RowKind::Thread { .. } => self.ask("title", "", &title, PromptKind::Rename { path }),
            RowKind::Event { index, .. } => {
                let index = *index;
                self.ask(format!("#{index}"), "", &title, PromptKind::EditEvent { path, index });
            }
            RowKind::Chat { file, .. } => {
                let session = self
                    .thread_at(&path)
                    .and_then(|t| t.chats.iter().find(|c| &c.file == file))
                    .map(|c| c.session.clone())
                    .unwrap_or_default();
                if session.is_empty() {
                    self.say(false, format!("{file} has no session id, open it in an editor"));
                    return Action::None;
                }
                return Action::RunChat { path, args: chat::resume_args(&session), label: title, created: None };
            }
        }
        Action::None
    }

    /// x: archive a thread, or delete an event or a chat. No question asked, because
    /// u takes any of it back.
    fn remove(&mut self, row: Option<&Row>) -> Action {
        let Some(row) = row else { return Action::None };
        let thread = row.path.clone();
        if self.bin.is_some() {
            self.say(false, "X purges, and asks you to type it out");
            return Action::None;
        }
        let cmd = match &row.kind {
            RowKind::Thread { .. } => Cmd::Archive { thread },
            RowKind::Event { index, .. } => Cmd::Delete { thread, index: Some(*index) },
            RowKind::Chat { file, .. } => Cmd::DeleteChat { thread, file: file.clone() },
        };
        let action = self.run_cmd(cmd);
        if let Some((true, text)) = &mut self.note {
            text.push_str(" · u to undo");
        }
        action
    }

    /// X: delete a thread into the trash, or purge one out of a bin for good. Both
    /// make you type the word, so neither can happen by leaning on a key.
    fn destroy(&mut self, row: Option<&Row>) {
        let Some(row) = row else { return };
        let thread = row.path.clone();
        let title = &row.title;
        if let Some(bin) = self.bin {
            if row.depth > 0 {
                self.say(false, "purge the whole thread, not a piece of it");
                return;
            }
            let label = format!("purge \"{title}\" from the {} for good", bin.word());
            let cmd = Cmd::Purge { bin: Some(bin), name: thread };
            self.ask(label, "type purge · there is no undo", "", PromptKind::Confirm { word: "purge", cmd });
            return;
        }
        if !matches!(row.kind, RowKind::Thread { .. }) {
            self.say(false, "x deletes an event or a chat; X is for a whole thread");
            return;
        }
        let label = format!("delete \"{title}\"{}", self.inside_note(&thread));
        let cmd = Cmd::Delete { thread, index: None };
        self.ask(label, "type delete · it goes to the trash, T to look, u to undo", "", PromptKind::Confirm { word: "delete", cmd });
    }

    /// " and 2 threads + 1 chat inside", or nothing when it is a leaf.
    fn inside_note(&self, path: &str) -> String {
        let Some(t) = self.thread_at(path) else { return String::new() };
        let counts = [(t.count() - 1, "thread"), (t.walk().iter().map(|x| x.chats.len()).sum(), "chat")];
        let parts: Vec<String> = counts
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, word)| format!("{n} {word}{}", plural(*n)))
            .collect();
        if parts.is_empty() {
            String::new()
        } else {
            format!(" and {} inside", parts.join(" + "))
        }
    }

    /// Which event a row is, when it is one. Threads and chats mark the thread.
    fn event_index(&self, r: &Row) -> Option<usize> {
        match r.kind {
            RowKind::Event { index, .. } => Some(index),
            _ => None,
        }
    }

    /// The thread under the cursor and the path it currently sits in. Only threads
    /// can be re-parented, so anything else is None.
    fn thread_row(&self, rows: &[Row], i: usize) -> Option<(String, String)> {
        let row = rows.get(i).filter(|r| matches!(r.kind, RowKind::Thread { .. }))?;
        Some((row.path.clone(), parent_path(&row.path).to_string()))
    }

    /// Tab: become a child of the thread just above at the same level.
    fn indent(&mut self, rows: &[Row], i: usize) {
        let Some((path, parent)) = self.thread_row(rows, i) else { return };
        let siblings: &[Thread] = match parent.as_str() {
            "" => &self.tree,
            p => self.thread_at(p).map(|t| t.children.as_slice()).unwrap_or_default(),
        };
        let Some(at) = siblings.iter().position(|s| s.path == path) else { return };
        if at == 0 {
            self.say(false, "nothing above to move under");
            return;
        }
        let target = siblings[at - 1].path.clone();
        self.expanded.insert(target.clone());
        self.run_cmd(Cmd::Move { thread: path, parent: Some(target) });
    }

    /// Shift+Tab: step out to sit beside the current parent.
    fn outdent(&mut self, rows: &[Row], i: usize) {
        let Some((path, parent)) = self.thread_row(rows, i) else { return };
        if parent.is_empty() {
            self.say(false, "already at the top level");
            return;
        }
        let grand = parent_path(&parent).to_string();
        self.run_cmd(Cmd::Move { thread: path, parent: Some(grand).filter(|g| !g.is_empty()) });
    }
}
