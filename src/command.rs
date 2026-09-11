//! What a command *does*. One grammar, two doors: the keys in the app and the CLI
//! both build a `Cmd` and send it here.

use crate::duration;
use crate::format::ThreadFile;
use crate::model::{join_path, walk_all, Event, Kind, Mark, Status, Thread};
use crate::reference::{resolve, Here};
use crate::store;
use anyhow::{bail, Result};
use chrono::{DateTime, Local};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Wait {
    /// No wait clause at all.
    None,
    /// Waiting, with no deadline.
    Open,
    For(chrono::Duration),
}

/// What a command did, beyond the sentence it reports. The app follows this instead
/// of reading the sentence back apart.
#[derive(Default)]
pub enum Effect {
    #[default]
    None,
    /// A thread was created here.
    Created(String),
    /// A thread now lives here.
    Moved(String),
    /// A chat file was written and is waiting to be launched.
    Chat { path: String, file: String, session: String, title: String },
}

pub struct Outcome {
    pub message: String,
    pub effect: Effect,
}

impl From<String> for Outcome {
    fn from(message: String) -> Outcome {
        Outcome { message, effect: Effect::None }
    }
}

#[derive(Debug)]
pub enum Cmd {
    New { parent: Option<String>, title: String, text: Option<String>, wait: Wait },
    Event { thread: String, text: String, wait: Wait },
    Wait { thread: String, wait: Wait },
    Done { thread: String, text: Option<String> },
    Edit { thread: String, index: usize, text: String },
    Rename { thread: String, title: String },
    Move { thread: String, parent: Option<String> },
    /// Swap places with the sibling above (-1) or below (+1).
    Reorder { thread: String, delta: i32 },
    /// Push an existing deadline out or pull it in.
    Snooze { thread: String, by: chrono::Duration },
    Delete { thread: String, index: Option<usize> },
    /// Swipe a highlighter over a thread's title, or over one of its events.
    Mark { thread: String, index: Option<usize>, mark: Option<Mark> },
    /// Write a new chat file, ready for the caller to launch.
    Chat { thread: String, title: String },
    DeleteChat { thread: String, file: String },
    Restore { name: String },
    Purge { name: String },
}

fn push_wait(events: &mut Vec<Event>, wait: Wait, now: DateTime<Local>) -> String {
    let (text, due) = match wait {
        Wait::For(d) => (duration::format(d), Some(now + d)),
        _ => (String::new(), None),
    };
    events.push(Event { kind: Kind::Wait, text, at: now, due, fired: None, mark: None });
    events.last().map(Event::label).unwrap_or_default()
}

/// The kind a new event should take: the first thing said in a log is the problem.
fn kind_for(events: &[Event]) -> Kind {
    if events.iter().any(|e| e.kind != Kind::Wait) {
        Kind::Action
    } else {
        Kind::Origin
    }
}

fn load(t: &Thread) -> ThreadFile {
    store::read_thread_file(&store::dir_of(&t.path))
}

fn save(t: &Thread, file: &ThreadFile) -> Result<()> {
    store::write_thread_file(&store::dir_of(&t.path), file)
}

/// Resolve a reference, read its file, change it, write it back. Nothing is written
/// if the change fails.
fn edit(
    tree: &[Thread],
    reference: &str,
    here: &Here,
    change: impl FnOnce(&Thread, &mut ThreadFile) -> Result<String>,
) -> Result<Outcome> {
    let t = resolve(tree, reference, here)?;
    let mut file = load(t);
    let message = change(t, &mut file)?;
    save(t, &file)?;
    Ok(message.into())
}

fn no_such_event(path: &str, n: usize, index: usize) -> anyhow::Error {
    anyhow::anyhow!("{path} has {n} event{}, no #{index}", crate::model::plural(n))
}

/// Applies one line to the folders under the root. Returns a one-line summary.
pub fn apply(line: &str, now: DateTime<Local>, here: &Here) -> Result<Outcome> {
    let mut cmd = crate::grammar::parse(line)?;
    let tree = store::read_tree()?;
    // "new hires: they replied" is an event when a thread called "new hires"
    // already exists, not a second thread called "hires"
    if let Cmd::New { parent: None, title, text: Some(text), wait } = &cmd {
        let reference = format!("new {title}");
        if resolve(&tree, &reference, here).is_ok() {
            cmd = Cmd::Event { thread: reference, text: text.clone(), wait: *wait };
        }
    }
    run(cmd, &tree, now, here)
}

/// The same work, from a command the app built directly, so nothing has to survive
/// a round trip through the grammar.
pub fn apply_cmd(cmd: Cmd, now: DateTime<Local>, here: &Here) -> Result<Outcome> {
    let tree = store::read_tree()?;
    run(cmd, &tree, now, here)
}

fn run(cmd: Cmd, tree: &[Thread], now: DateTime<Local>, here: &Here) -> Result<Outcome> {
    match cmd {
        Cmd::New { parent, title, text, wait } => {
            let parent_path = match parent {
                Some(p) => resolve(tree, &p, here)?.path.clone(),
                None => String::new(),
            };
            let parent_dir = store::dir_of(&parent_path);
            let name = store::free_name(&parent_dir, &title);
            let mut events = Vec::new();
            if let Some(text) = text {
                events.push(Event { kind: Kind::Origin, text, at: now, due: None, fired: None, mark: None });
            }
            if wait != Wait::None {
                push_wait(&mut events, wait, now);
            }
            let file = ThreadFile { title, meta: Default::default(), events, notes: String::new() };
            store::write_thread_file(&parent_dir.join(&name), &file)?;
            let path = join_path(&parent_path, &name);
            Ok(Outcome { message: format!("new {path}"), effect: Effect::Created(path) })
        }
        Cmd::Event { thread, text, wait } => edit(tree, &thread, here, |t, file| {
            let reopened = t.status(now) == Status::Closed;
            file.events.push(Event {
                kind: kind_for(&file.events),
                text: text.clone(),
                at: now,
                due: None,
                fired: None,
                mark: None,
            });
            let tail = if wait == Wait::None {
                String::new()
            } else {
                format!(", {}", push_wait(&mut file.events, wait, now))
            };
            Ok(format!("{}: {text}{tail}{}", t.path, if reopened { " (reopened)" } else { "" }))
        }),
        Cmd::Wait { thread, wait } => edit(tree, &thread, here, |t, file| {
            let reopened = t.status(now) == Status::Closed;
            let text = push_wait(&mut file.events, wait, now);
            Ok(format!("{}: {text}{}", t.path, if reopened { " (reopened)" } else { "" }))
        }),
        Cmd::Done { thread, text } => edit(tree, &thread, here, |t, file| {
            if t.last().map(|e| e.kind) == Some(Kind::Done) {
                bail!("{} is already closed", t.path);
            }
            file.events.push(Event {
                kind: Kind::Done,
                text: text.unwrap_or_default(),
                at: now,
                due: None,
                fired: None,
                mark: None,
            });
            let open = t.children.iter().filter(|c| c.status(now).is_open()).count();
            let note = if open > 0 { format!(", {open} still open inside") } else { String::new() };
            let said = file.events.last().map(|e| e.label()).unwrap_or_default();
            Ok(format!("{}: {said}{note}", t.path))
        }),
        Cmd::Edit { thread, index, text } => edit(tree, &thread, here, |t, file| {
            let n = file.events.len();
            let e = file.events.get_mut(index.wrapping_sub(1)).ok_or_else(|| no_such_event(&t.path, n, index))?;
            if e.kind == Kind::Wait {
                // a wait's text is its duration, so editing it has to move the deadline too
                let arg = text.trim().trim_start_matches("wait").trim();
                if arg.is_empty() {
                    e.text = String::new();
                    e.due = None;
                } else {
                    let d = duration::parse(arg)?;
                    e.text = duration::format(d);
                    e.due = Some(e.at + d);
                }
                e.fired = None;
            } else {
                e.text = text.clone();
            }
            Ok(format!("{}: #{index} is now \"{}\"", t.path, e.label()))
        }),
        Cmd::Rename { thread, title } => edit(tree, &thread, here, |t, file| {
            file.title = title.clone();
            Ok(format!("{}: renamed to \"{title}\"", t.path))
        }),
        Cmd::Move { thread, parent } => {
            let t = resolve(tree, &thread, here)?;
            let parent_path = match parent {
                Some(p) => resolve(tree, &p, here)?.path.clone(),
                None => String::new(),
            };
            let from = t.path.clone();
            let to = store::move_dir(&from, &parent_path)?;
            if to == from {
                return Ok(format!("{from} is already there").into());
            }
            Ok(Outcome { message: format!("moved {from} to {to}"), effect: Effect::Moved(to) })
        }
        Cmd::Reorder { thread, delta } => {
            let t = resolve(tree, &thread, here)?;
            let way = if delta < 0 { "up" } else { "down" };
            Ok(if store::reorder(&t.path, delta)? {
                format!("moved {} {way}", t.path)
            } else {
                format!("{} is already at the {}", t.path, if delta < 0 { "top" } else { "bottom" })
            }
            .into())
        }
        Cmd::Snooze { thread, by } => edit(tree, &thread, here, |t, file| {
            let last = file.events.last_mut().filter(|e| e.kind == Kind::Wait);
            let Some(last) = last else { bail!("{} is not waiting, start a wait first", t.path) };
            let Some(due) = last.due else { bail!("{} is waiting with no deadline to move", t.path) };
            // a deadline cannot land before the wait began; pulling harder just makes it due now
            let moved = (due + by).max(last.at);
            last.due = Some(moved);
            last.fired = None;
            last.text = duration::format(moved - last.at);
            let way = if by < chrono::Duration::zero() { "pulled in" } else { "pushed out" };
            Ok(format!("{}: {way} to {}", t.path, duration::due_label(moved, now)))
        }),
        Cmd::Chat { thread, title } => {
            let t = resolve(tree, &thread, here)?;
            let path = t.path.clone();
            let c = crate::chat::create(&path, &title, now)?;
            Ok(Outcome {
                message: format!("{path}/{}/{}", store::CHATS_DIR, c.file),
                effect: Effect::Chat { path, file: c.file, session: c.session, title: c.title },
            })
        }
        Cmd::DeleteChat { thread, file } => {
            let t = resolve(tree, &thread, here)?;
            store::remove_chat(&t.path, &file)?;
            Ok(format!("deleted chat {file}").into())
        }
        Cmd::Mark { thread, index, mark } => edit(tree, &thread, here, |t, file| {
            let what = match index {
                None => {
                    file.meta.mark = mark;
                    format!("\"{}\"", file.title)
                }
                Some(i) => {
                    let n = file.events.len();
                    let e = file.events.get_mut(i.wrapping_sub(1)).ok_or_else(|| no_such_event(&t.path, n, i))?;
                    e.mark = mark;
                    format!("#{i}")
                }
            };
            Ok(match mark {
                Some(m) => format!("{}: {what} highlighted {}", t.path, m.word()),
                None => format!("{}: {what} unhighlighted", t.path),
            })
        }),
        Cmd::Restore { name } => Ok(format!("restored {}", store::restore(&name)?).into()),
        Cmd::Purge { name } => {
            store::purge(&name)?;
            Ok(format!("purged {name}").into())
        }
        Cmd::Delete { thread, index } => {
            let t = resolve(tree, &thread, here)?;
            let Some(index) = index else {
                let inside = t.count() - 1;
                let path = t.path.clone();
                store::archive(&path, now)?;
                let note = if inside > 0 { format!(" and {inside} inside it") } else { String::new() };
                return Ok(format!("archived {path}{note}").into());
            };
            edit(tree, &thread, here, |t, file| {
                let n = file.events.len();
                if index == 0 || index > n {
                    return Err(no_such_event(&t.path, n, index));
                }
                let removed = file.events.remove(index - 1);
                // a log starts with its problem: if the origin went, the first action takes over
                if !file.events.iter().any(|e| e.kind == Kind::Origin) {
                    if let Some(e) = file.events.iter_mut().find(|e| e.kind == Kind::Action) {
                        e.kind = Kind::Origin;
                    }
                }
                Ok(format!("{}: deleted #{index} \"{}\"", t.path, removed.label()))
            })
        }
    }
}

/// Marks passed deadlines once. Returns one line per thread that just crossed its due.
pub fn check_timers(now: DateTime<Local>) -> Result<Vec<String>> {
    let tree = store::read_tree()?;
    let mut fired = Vec::new();
    for t in walk_all(&tree) {
        let Some(last) = t.last() else { continue };
        if last.kind != Kind::Wait || last.fired.is_some() {
            continue;
        }
        let Some(due) = last.due else { continue };
        if due > now {
            continue;
        }
        let mut file = load(t);
        match file.events.last_mut() {
            Some(e) if e.kind == Kind::Wait => e.fired = Some(now),
            _ => continue,
        }
        save(t, &file)?;
        let waited = duration::format(due - last.at);
        fired.push(format!("{}: no reply in {waited}", t.path));
    }
    Ok(fired)
}


