//! The view model: the tree flattened into the lines currently on screen. Filtering
//! and the view chooser both narrow it here, so the header can never disagree with
//! the list.

use crate::app::App;
use crate::model::{Kind, Mark, Status, Thread};

pub enum RowKind {
    Thread {
        status: Status,
        note: Option<String>,
        /// the note is this thread's own missed deadline, not a child's
        note_alarm: bool,
        children: usize,
        chats: usize,
        open: bool,
        deep: bool,
    },
    Event { index: usize, kind: Kind, when: String },
    Chat { file: String, when: String },
}

pub struct Row {
    pub key: String,
    pub path: String,
    /// The highlighter swiped over this line, if any.
    pub mark: Option<Mark>,
    /// The thread's title, the event's label, or the chat's title.
    pub title: String,
    pub depth: usize,
    pub kind: RowKind,
}

impl App {
    /// Does this thread, or anything under it, mention `q`?
    fn any_match(t: &Thread, q: &str) -> bool {
        t.title.to_lowercase().contains(q)
            || t.path.to_lowercase().contains(q)
            || t.events.iter().any(|e| e.text.to_lowercase().contains(q))
            || t.chats.iter().any(|c| c.title.to_lowercase().contains(q))
            || t.children.iter().any(|c| App::any_match(c, q))
    }

    pub fn source(&self) -> &[Thread] {
        if self.bin.is_some() {
            &self.binned
        } else {
            &self.tree
        }
    }

    /// Has this thread, and everything under it, stopped moving?
    fn is_cold(&self, t: &Thread) -> bool {
        t.is_cold(self.now, self.config.cools_after)
    }

    /// Cold and current are two halves of the same tree. Among the cold a live parent
    /// still comes along, or a cold thread filed under it could not be reached.
    fn is_current(&self, t: &Thread) -> bool {
        if self.view.shows_cold() {
            self.is_cold(t) || t.children.iter().any(|c| self.is_current(c))
        } else {
            !self.is_cold(t)
        }
    }

    /// Does this thread, or anything under it, belong in the current view?
    fn in_view(&self, t: &Thread) -> bool {
        self.view.accepts(t.status(self.now)) || t.children.iter().any(|c| self.in_view(c))
    }

    pub fn rows(&self) -> Vec<Row> {
        let q = self.filter.as_ref().map(|f| f.text.to_lowercase()).unwrap_or_default();
        let mut out = Vec::new();
        for t in self.source() {
            self.push_rows(t, 0, &q, &mut out);
        }
        out
    }

    fn push_rows(&self, t: &Thread, depth: usize, q: &str, out: &mut Vec<Row>) {
        if !q.is_empty() && !App::any_match(t, q) {
            return;
        }
        if self.bin.is_none() && !self.in_view(t) {
            return;
        }
        // a filter searches everything, so it reaches what has gone cold too
        if self.bin.is_none() && q.is_empty() && !self.is_current(t) {
            return;
        }
        let open = self.expanded.contains(&t.path);
        let deep = t.children.len() + t.events.len() + t.chats.len() > 0;
        out.push(Row {
            key: t.path.clone(),
            path: t.path.clone(),
            mark: t.meta.mark,
            title: t.title.clone(),
            depth,
            kind: RowKind::Thread {
                status: t.status(self.now),
                note: t.note(self.now),
                note_alarm: t.own_status(self.now) == Some(Status::Overdue),
                children: t.children.len(),
                chats: t.chats.len(),
                open,
                deep,
            },
        });
        // while filtering, keep walking down so a deep match is still reachable
        let show_children = open || !q.is_empty();
        if open {
            for (i, e) in t.events.iter().enumerate() {
                out.push(Row {
                    key: format!("{}#{}", t.path, i + 1),
                    path: t.path.clone(),
                    mark: e.mark,
                    title: e.label(),
                    depth: depth + 1,
                    kind: RowKind::Event {
                        index: i + 1,
                        kind: e.kind,
                        when: crate::duration::ago(e.at, self.now),
                    },
                });
            }
            for c in &t.chats {
                out.push(Row {
                    key: format!("{}@{}", t.path, c.file),
                    path: t.path.clone(),
                    mark: None,
                    title: c.title.clone(),
                    depth: depth + 1,
                    kind: RowKind::Chat {
                        file: c.file.clone(),
                        when: c.created.map(|t| crate::duration::ago(t, self.now)).unwrap_or_default(),
                    },
                });
            }
        }
        if show_children {
            for c in &t.children {
                self.push_rows(c, depth + 1, q, out);
            }
        }
    }

    pub fn index(&self, rows: &[Row]) -> usize {
        self.cursor
            .as_ref()
            .and_then(|c| rows.iter().position(|r| &r.key == c))
            .unwrap_or(0)
    }

    /// Counts the threads on screen right now, so the header can never disagree with
    /// the rows, whatever view or filter is on.
    pub fn counts(rows: &[Row]) -> (usize, usize) {
        rows.iter()
            .filter_map(|r| match r.kind {
                RowKind::Thread { status, .. } => Some(status),
                _ => None,
            })
            .fold((0, 0), |(open, overdue), s| {
                (open + s.is_open() as usize, overdue + (s == Status::Overdue) as usize)
            })
    }

    pub(crate) fn top_row(&self) -> Option<String> {
        self.rows().first().map(|r| r.key.clone())
    }
}
