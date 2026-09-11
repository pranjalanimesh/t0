//! What a command *does*, and how to take it back. One grammar, two doors: the keys
//! in the app and the CLI both build a `Cmd` and send it here.

use crate::duration;
use crate::format::{serialize_thread, ThreadFile};
use crate::model::{join_path, walk_all, Event, Kind, Mark, Status, Thread};
use crate::reference::{resolve, Here};
use crate::store::{self, Bin};
use anyhow::{bail, Result};
use chrono::{DateTime, Local};
use std::fs;
use std::path::PathBuf;

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

/// One thing to put back. A command records these as it goes, so `u` can take it
/// back later without the app having to know what each command did.
pub enum Step {
    /// A file this command rewrote or removed. `after` is what it left behind, and
    /// the old bytes only go back if that is still what is there.
    File { path: PathBuf, before: Option<String>, after: Option<String> },
    /// A command whose opposite is another command: a move back, a restore.
    Cmd(Cmd),
}

pub struct Outcome {
    pub message: String,
    pub effect: Effect,
    /// Empty when there is nothing to take back: a purge, a chat that was launched.
    pub undo: Vec<Step>,
}

impl From<String> for Outcome {
    fn from(message: String) -> Outcome {
        Outcome { message, effect: Effect::None, undo: Vec::new() }
    }
}

#[derive(Debug, Clone)]
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
    /// A thread goes into the archive. It comes back with `restore`.
    Archive { thread: String },
    /// A thread goes into the trash; an event line is just gone. Both undo.
    Delete { thread: String, index: Option<usize> },
    /// Swipe a highlighter over a thread's title, or over one of its events.
    Mark { thread: String, index: Option<usize>, mark: Option<Mark> },
    /// Write a new chat file, ready for the caller to launch.
    Chat { thread: String, title: String },
    DeleteChat { thread: String, file: String },
    /// No bin means look in both, the archive first.
    Restore { bin: Option<Bin>, name: String },
    Purge { bin: Option<Bin>, name: String },
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
/// if the change fails. The old bytes are kept so the change can be undone.
fn edit(
    tree: &[Thread],
    reference: &str,
    here: &Here,
    change: impl FnOnce(&Thread, &mut ThreadFile) -> Result<String>,
) -> Result<Outcome> {
    let t = resolve(tree, reference, here)?;
    let path = store::dir_of(&t.path).join(store::THREAD_FILE);
    let before = fs::read_to_string(&path).ok();
    let mut file = load(t);
    let message = change(t, &mut file)?;
    save(t, &file)?;
    let step = Step::File { path, before, after: Some(serialize_thread(&file)) };
    Ok(Outcome { message, effect: Effect::None, undo: vec![step] })
}

/// Take a thread out of the tree into a bin. It comes back with `restore`, or `u`.
fn stash(tree: &[Thread], reference: &str, here: &Here, bin: Bin, now: DateTime<Local>) -> Result<Outcome> {
    let t = resolve(tree, reference, here)?;
    let inside = t.count() - 1;
    let path = t.path.clone();
    let name = store::stash(&path, bin, now)?;
    let note = if inside > 0 { format!(" and {inside} inside it") } else { String::new() };
    let verb = match bin {
        Bin::Archive => "archived",
        Bin::Trash => "deleted",
    };
    Ok(Outcome {
        message: format!("{verb} {path}{note}"),
        effect: Effect::None,
        undo: vec![Step::Cmd(Cmd::Restore { bin: Some(bin), name })],
    })
}

/// The bin a name was given, or the one that holds it.
fn bin_holding(bin: Option<Bin>, name: &str) -> Result<Bin> {
    bin.or_else(|| store::bin_of(name))
        .ok_or_else(|| anyhow::anyhow!("nothing called \"{name}\" in the archive or the trash"))
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
            Ok(Outcome {
                message: format!("new {path}"),
                effect: Effect::Created(path.clone()),
                // fresh, but not nothing: it goes to the trash rather than away
                undo: vec![Step::Cmd(Cmd::Delete { thread: path, index: None })],
            })
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
            let back = Cmd::Move {
                thread: to.clone(),
                parent: Some(crate::model::parent_path(&from).to_string()).filter(|p| !p.is_empty()),
            };
            Ok(Outcome { message: format!("moved {from} to {to}"), effect: Effect::Moved(to), undo: vec![Step::Cmd(back)] })
        }
        Cmd::Reorder { thread, delta } => {
            let t = resolve(tree, &thread, here)?;
            let way = if delta < 0 { "up" } else { "down" };
            if !store::reorder(&t.path, delta)? {
                return Ok(format!("{} is already at the {}", t.path, if delta < 0 { "top" } else { "bottom" }).into());
            }
            let back = Cmd::Reorder { thread: t.path.clone(), delta: -delta };
            Ok(Outcome { message: format!("moved {} {way}", t.path), effect: Effect::None, undo: vec![Step::Cmd(back)] })
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
                undo: Vec::new(),
            })
        }
        Cmd::DeleteChat { thread, file } => {
            let t = resolve(tree, &thread, here)?;
            let path = store::dir_of(&t.path).join(store::CHATS_DIR).join(&file);
            let before = fs::read_to_string(&path).ok();
            store::remove_chat(&t.path, &file)?;
            let step = Step::File { path, before, after: None };
            Ok(Outcome { message: format!("deleted chat {file}"), effect: Effect::None, undo: vec![step] })
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
        Cmd::Restore { bin, name } => {
            let bin = bin_holding(bin, &name)?;
            let path = store::restore(bin, &name)?;
            let back = match bin {
                Bin::Archive => Cmd::Archive { thread: path.clone() },
                Bin::Trash => Cmd::Delete { thread: path.clone(), index: None },
            };
            Ok(Outcome {
                message: format!("restored {path} from the {}", bin.word()),
                effect: Effect::None,
                undo: vec![Step::Cmd(back)],
            })
        }
        Cmd::Purge { bin, name } => {
            let bin = bin_holding(bin, &name)?;
            store::purge(bin, &name)?;
            Ok(format!("purged {name}, gone for good").into())
        }
        Cmd::Archive { thread } => stash(tree, &thread, here, Bin::Archive, now),
        Cmd::Delete { thread, index } => {
            let Some(index) = index else {
                return stash(tree, &thread, here, Bin::Trash, now);
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

/// Take a command back, last step first. A file only goes back if it still holds
/// what the command wrote, so an edit made since by a chat or the CLI is kept.
pub fn revert(steps: &[Step], now: DateTime<Local>) -> Result<()> {
    for step in steps.iter().rev() {
        match step {
            Step::File { path, before, after } => {
                if fs::read_to_string(path).ok() != *after {
                    let shown = path.strip_prefix(store::root()).unwrap_or(path);
                    bail!("{} changed since, left as it is", shown.display());
                }
                match before {
                    Some(text) => fs::write(path, text)?,
                    None => fs::remove_file(path)?,
                }
            }
            Step::Cmd(cmd) => {
                apply_cmd(cmd.clone(), now, &Here::default())?;
            }
        }
    }
    Ok(())
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



#[cfg(test)]
mod tests {
    use super::*;

    /// One test, because T0_ROOT is process-wide and tests run in parallel.
    #[test]
    fn undo_puts_things_back() {
        let root = std::env::temp_dir().join(format!("t0-undo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        std::env::set_var("T0_ROOT", &root);
        store::ensure_root().unwrap();
        let now = Local::now();
        let here = Here::default();
        let text = |t: &str| fs::read_to_string(root.join(t).join(store::THREAD_FILE)).ok();

        // an event: the file goes back to its old bytes
        apply("new alpha: first", now, &here).unwrap();
        let before = text("alpha");
        let o = apply("alpha: second", now, &here).unwrap();
        assert_ne!(text("alpha"), before);
        revert(&o.undo, now).unwrap();
        assert_eq!(text("alpha"), before);

        // an event, then someone else edits the file: undo refuses, the edit stays
        let o = apply("alpha: third", now, &here).unwrap();
        fs::write(root.join("alpha").join(store::THREAD_FILE), "# alpha\n\nhand edited\n").unwrap();
        assert!(revert(&o.undo, now).is_err());
        assert_eq!(text("alpha").unwrap(), "# alpha\n\nhand edited\n");

        // archive and delete both come back, into the same place
        let o = apply("archive alpha", now, &here).unwrap();
        assert!(text("alpha").is_none());
        revert(&o.undo, now).unwrap();
        assert!(text("alpha").is_some());
        let o = apply("delete alpha", now, &here).unwrap();
        assert!(root.join(".trash/alpha").is_dir());
        revert(&o.undo, now).unwrap();
        assert!(text("alpha").is_some() && !root.join(".trash/alpha").exists());

        // a restore undoes back into the bin it came from
        apply("delete alpha", now, &here).unwrap();
        let o = apply("restore alpha", now, &here).unwrap();
        revert(&o.undo, now).unwrap();
        assert!(root.join(".trash/alpha").is_dir());
        apply("restore alpha", now, &here).unwrap();

        // new undoes into the trash, not into nothing
        let o = apply("new beta: b", now, &here).unwrap();
        revert(&o.undo, now).unwrap();
        assert!(root.join(".trash/beta").is_dir());

        // a move goes back under its old parent
        apply("new alpha/inner: i", now, &here).unwrap();
        let o = apply("move inner top", now, &here).unwrap();
        assert!(text("inner").is_some());
        revert(&o.undo, now).unwrap();
        assert!(text("alpha/inner").is_some());

        // a purge has nothing to undo
        apply("delete alpha", now, &here).unwrap();
        assert!(apply("purge alpha", now, &here).unwrap().undo.is_empty());
        let _ = fs::remove_dir_all(&root);
    }
}
