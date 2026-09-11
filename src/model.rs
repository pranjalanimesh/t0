//! The domain: threads, the events logged against them, and the status those
//! events add up to. No I/O, no rendering.

use crate::duration;
use chrono::{DateTime, Duration, Local};

/// "" for one, "s" for the rest.
pub fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Origin,
    Action,
    Wait,
    Done,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            // "origin" is what older files say; both read, t0 is what gets written
            "t0" | "origin" => Some(Kind::Origin),
            "action" => Some(Kind::Action),
            "wait" => Some(Kind::Wait),
            "done" => Some(Kind::Done),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::Origin => "t0",
            Kind::Action => "action",
            Kind::Wait => "wait",
            Kind::Done => "done",
        }
    }
}

/// A highlighter swiped across a line. A few fixed pens, the way a real one comes in
/// a few colours: this is for salience, not for filing things away.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    Yellow,
    Green,
    Pink,
    Blue,
}

pub const MARKS: &[Mark] = &[Mark::Yellow, Mark::Green, Mark::Pink, Mark::Blue];

impl Mark {
    pub fn parse(s: &str) -> Option<Mark> {
        MARKS.iter().copied().find(|m| m.word() == s)
    }

    pub fn word(self) -> &'static str {
        match self {
            Mark::Yellow => "yellow",
            Mark::Green => "green",
            Mark::Pink => "pink",
            Mark::Blue => "blue",
        }
    }

    /// Swipe again for the next pen; past the last one the highlight comes off.
    pub fn cycle(current: Option<Mark>) -> Option<Mark> {
        let Some(m) = current else { return MARKS.first().copied() };
        let i = MARKS.iter().position(|x| *x == m).unwrap_or(0);
        MARKS.get(i + 1).copied()
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    pub kind: Kind,
    /// For a wait this is the duration label ("2d"), empty when there is no deadline.
    pub text: String,
    pub at: DateTime<Local>,
    pub due: Option<DateTime<Local>>,
    /// Set once the timer has notified about a passed deadline.
    pub fired: Option<DateTime<Local>>,
    pub mark: Option<Mark>,
}

impl Event {
    /// What the log line reads as on screen.
    pub fn label(&self) -> String {
        match self.kind {
            Kind::Wait if self.text.is_empty() => "wait".into(),
            Kind::Wait => format!("wait {}", self.text),
            Kind::Done if self.text.is_empty() => "done".into(),
            _ => self.text.clone(),
        }
    }
}

/// Machine-owned fields kept in thread.md, invisible when the file is rendered.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    /// Where this thread sits among its siblings once you have moved it by hand.
    pub order: Option<u32>,
    /// Archived threads only: the path they came from, so they can go back.
    pub from: Option<String>,
    pub archived: Option<DateTime<Local>>,
    pub mark: Option<Mark>,
}

#[derive(Clone, Debug)]
pub struct Chat {
    pub file: String,
    pub session: String,
    pub title: String,
    pub created: Option<DateTime<Local>>,
}

/// One directory under the root. Children are its subdirectories.
#[derive(Clone, Debug)]
pub struct Thread {
    /// Relative to the root, e.g. "infra/aws". Empty for the root itself.
    pub path: String,
    /// Directory name, the last path segment.
    pub name: String,
    pub title: String,
    pub meta: Meta,
    pub events: Vec<Event>,
    pub chats: Vec<Chat>,
    pub children: Vec<Thread>,
}

/// Ordered quietest to loudest: a parent takes the loudest state inside it. Work you
/// have started is quieter than work you have not touched, which is the whole point
/// of separating the two.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Status {
    Closed,
    Waiting,
    Doing,
    Yours,
    Overdue,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Yours => "yours",
            Status::Doing => "doing",
            Status::Waiting => "waiting",
            Status::Overdue => "overdue",
            Status::Closed => "closed",
        }
    }

    /// The box beside a thread, read as a to-do: empty until it is closed.
    pub fn glyph(self) -> &'static str {
        match self {
            Status::Closed => "▣ ",
            Status::Doing => "◧ ",
            _ => "▢ ",
        }
    }

    pub fn is_open(self) -> bool {
        self != Status::Closed
    }
}

impl Thread {
    pub fn last(&self) -> Option<&Event> {
        self.events.last()
    }

    /// The last event that says something: waits carry no words of their own.
    pub fn said(&self) -> Option<&Event> {
        self.events.iter().rev().find(|e| e.kind != Kind::Wait && !e.text.is_empty())
    }

    /// The one line beside a thread's title: the deadline while waiting, else how long
    /// since anything happened.
    pub fn note(&self, now: DateTime<Local>) -> Option<String> {
        let last = self.last()?;
        Some(match (last.kind, last.due) {
            (Kind::Wait, Some(due)) => duration::due_label(due, now),
            _ => duration::ago(last.at, now),
        })
    }

    /// What this thread alone says, ignoring anything nested inside it.
    pub fn own_status(&self, now: DateTime<Local>) -> Option<Status> {
        let last = self.last()?;
        Some(match last.kind {
            Kind::Done => Status::Closed,
            Kind::Wait => match last.due {
                Some(due) if due < now => Status::Overdue,
                _ => Status::Waiting,
            },
            // something has been done about it; a bare problem has not been started
            Kind::Action => Status::Doing,
            Kind::Origin => Status::Yours,
        })
    }

    /// A thread cannot be quieter than what is still open inside it, so a closed
    /// parent with a live child still shows the child's state.
    pub fn status(&self, now: DateTime<Local>) -> Status {
        // None sorts below Some, so a thread with no events takes whatever is inside it
        let inside = self.children.iter().map(|c| c.status(now)).max();
        self.own_status(now).max(inside).unwrap_or(Status::Yours)
    }

    /// The last time anything happened anywhere in this subtree.
    pub fn touched(&self) -> Option<DateTime<Local>> {
        self.events
            .iter()
            .map(|e| e.at)
            .chain(self.children.iter().filter_map(Thread::touched))
            .max()
    }

    /// Gone cold: nothing here or under it has moved in a long while. A thread
    /// nothing has ever happened to is not cold, it is just empty, so a blank one you
    /// made a minute ago does not vanish.
    pub fn is_cold(&self, now: DateTime<Local>, after: Duration) -> bool {
        self.touched().is_some_and(|at| now - at > after)
    }

    /// Every thread in this subtree, this one first.
    pub fn walk(&self) -> Vec<&Thread> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.walk());
        }
        out
    }

    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Thread::count).sum::<usize>()
    }
}

pub fn walk_all(tree: &[Thread]) -> Vec<&Thread> {
    tree.iter().flat_map(Thread::walk).collect()
}

pub fn find<'a>(tree: &'a [Thread], path: &str) -> Option<&'a Thread> {
    walk_all(tree).into_iter().find(|t| t.path == path)
}

/// What a view shows. Each one is a question you ask the tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Current,
    Open,
    Mine,
    Doing,
    Waiting,
    Overdue,
    Closed,
    /// Everything that stopped moving. Nothing else shows these.
    Cold,
}

pub const VIEWS: &[View] = &[
    View::Current,
    View::Open,
    View::Mine,
    View::Doing,
    View::Waiting,
    View::Overdue,
    View::Closed,
    View::Cold,
];

impl View {
    pub fn name(self) -> &'static str {
        match self {
            View::Current => "current",
            View::Open => "open",
            View::Mine => "my move",
            View::Doing => "doing",
            View::Waiting => "waiting",
            View::Overdue => "overdue",
            View::Closed => "closed",
            View::Cold => "cold",
        }
    }

    pub fn accepts(self, s: Status) -> bool {
        match self {
            View::Current | View::Cold => true,
            View::Open => s.is_open(),
            // started or not, the ball is still in your court
            View::Mine => s == Status::Yours || s == Status::Doing,
            View::Doing => s == Status::Doing,
            View::Waiting => s == Status::Waiting || s == Status::Overdue,
            View::Overdue => s == Status::Overdue,
            View::Closed => s == Status::Closed,
        }
    }

    /// Only this view shows what has stopped moving.
    pub fn shows_cold(self) -> bool {
        self == View::Cold
    }

    fn step(self, by: usize) -> View {
        let i = VIEWS.iter().position(|v| *v == self).unwrap_or(0);
        VIEWS[(i + by) % VIEWS.len()]
    }

    pub fn next(self) -> View {
        self.step(1)
    }

    pub fn prev(self) -> View {
        self.step(VIEWS.len() - 1)
    }
}

pub fn parent_path(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// The inverse of `parent_path`: a child of the root keeps its bare name.
pub fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}
