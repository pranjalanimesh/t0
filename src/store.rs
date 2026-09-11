//! The store on disk: one folder per thread, `thread.md` inside it. Everything
//! that reads or writes `~/.t0` goes through here.

use crate::format::{parse_chat, parse_thread, serialize_thread, ThreadFile};
use crate::model::{join_path, parent_path, Chat, Meta, Thread};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub const THREAD_FILE: &str = "thread.md";
pub const CHATS_DIR: &str = "chats";

/// Threads taken out of the tree land in one of these. Dot-prefixed, so the tree
/// walk already skips them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bin {
    /// Finished with, kept to look back on.
    Archive,
    /// Deleted. Stays until purged, so a wrong key can be taken back.
    Trash,
}

impl Bin {
    pub fn word(self) -> &'static str {
        match self {
            Bin::Archive => "archive",
            Bin::Trash => "trash",
        }
    }

    fn dir(self) -> PathBuf {
        root().join(match self {
            Bin::Archive => ".archive",
            Bin::Trash => ".trash",
        })
    }
}

/// Folder names a thread may not take, because the store already means something by them.
fn reserved(name: &str) -> bool {
    name == CHATS_DIR || name == THREAD_FILE || name.starts_with('.')
}

pub fn root() -> PathBuf {
    std::env::var_os("T0_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".t0")))
        .unwrap_or_else(|| PathBuf::from(".t0"))
}

pub fn dir_of(path: &str) -> PathBuf {
    if path.is_empty() {
        root()
    } else {
        root().join(path)
    }
}

/// A folder name for a title. Any script is fine; a title with nothing name-like
/// in it still gets a folder rather than an error.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    let id = out.trim_end_matches('-').to_string();
    if id.is_empty() {
        return "thread".into();
    }
    if reserved(&id) {
        format!("{id}-thread")
    } else {
        id
    }
}

/// A free name under `parent`: the base, then base-2, base-3 and so on.
pub fn free_named(parent: &Path, base: &str, ext: &str) -> String {
    let mut name = format!("{base}{ext}");
    let mut n = 2;
    while parent.join(&name).exists() {
        name = format!("{base}-{n}{ext}");
        n += 1;
    }
    name
}

/// A free directory name under `parent`, named after a title.
pub fn free_name(parent: &Path, title: &str) -> String {
    free_named(parent, &slug(title), "")
}

pub fn read_thread_file(dir: &Path) -> ThreadFile {
    let fallback = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    match fs::read_to_string(dir.join(THREAD_FILE)) {
        Ok(text) => parse_thread(&text, &fallback),
        Err(_) => ThreadFile { title: fallback, meta: Meta::default(), events: Vec::new(), notes: String::new() },
    }
}

/// Write to a sibling file and rename, so a reader never sees a half-written file.
/// The temp name carries this process's id, so two writers cannot take each other's.
pub fn write_thread_file(dir: &Path, t: &ThreadFile) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let file = dir.join(THREAD_FILE);
    let tmp = dir.join(format!(".{THREAD_FILE}.{}.tmp", std::process::id()));
    fs::write(&tmp, serialize_thread(t)).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &file).with_context(|| format!("saving {}", file.display()))?;
    Ok(())
}

/// Read a thread's file, change only its machine-owned fields, write it back.
fn update_meta(dir: &Path, f: impl FnOnce(&mut Meta)) -> Result<()> {
    let mut file = read_thread_file(dir);
    f(&mut file.meta);
    write_thread_file(dir, &file)
}

fn read_node(dir: &Path, path: &str) -> Thread {
    let file = read_thread_file(dir);
    let mut children: Vec<Thread> = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let child = entry.path();
            if reserved(&name) || !child.is_dir() {
                continue;
            }
            children.push(read_node(&child, &join_path(path, &name)));
        }
    }
    // where you put it if you moved it, otherwise oldest first
    children.sort_by(|a, b| {
        let key = |t: &Thread| (t.meta.order.unwrap_or(u32::MAX), t.events.first().map(|e| e.at));
        key(a).cmp(&key(b)).then_with(|| a.name.cmp(&b.name))
    });
    Thread {
        path: path.to_string(),
        name: dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        title: file.title,
        meta: file.meta,
        events: file.events,
        chats: read_chats(dir),
        children,
    }
}

pub fn read_chats(dir: &Path) -> Vec<Chat> {
    let chats_dir = dir.join(CHATS_DIR);
    let Ok(entries) = fs::read_dir(&chats_dir) else { return Vec::new() };
    let mut files: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|f| f.ends_with(".md") && !f.starts_with('.'))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|file| {
            let text = fs::read_to_string(chats_dir.join(&file)).unwrap_or_default();
            parse_chat(&file, &text)
        })
        .collect()
}

pub fn read_tree() -> Result<Vec<Thread>> {
    let root = root();
    fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    if fs::metadata(&root).map(|m| m.permissions().readonly()).unwrap_or(false) {
        bail!("{} is read only", root.display());
    }
    Ok(read_node(&root, "").children)
}

/// Cheap fingerprint of the tree on disk, so the app only reloads when something changed.
pub fn signature() -> u64 {
    fn walk(dir: &Path, acc: &mut u64, depth: usize) {
        // deep enough that no real tree reaches it, shallow enough to stop a symlink loop
        if depth > 64 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let secs = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                *acc = acc.wrapping_mul(31).wrapping_add(secs);
                *acc = acc.wrapping_mul(31).wrapping_add(name.len() as u64);
                if meta.is_dir() {
                    walk(&entry.path(), acc, depth + 1);
                }
            }
        }
    }
    let mut acc = 0u64;
    walk(&root(), &mut acc, 0);
    acc
}

/// Returns the thread's new path. Moving somewhere it already is does nothing.
pub fn move_dir(from_path: &str, to_parent: &str) -> Result<String> {
    let from = dir_of(from_path);
    let parent_dir = dir_of(to_parent);
    if parent_path(from_path) == to_parent {
        return Ok(from_path.to_string());
    }
    if parent_dir.starts_with(&from) {
        bail!("cannot move a thread inside itself");
    }
    let name = free_name(&parent_dir, &from.file_name().unwrap_or_default().to_string_lossy());
    fs::create_dir_all(&parent_dir).with_context(|| format!("creating {}", parent_dir.display()))?;
    fs::rename(&from, parent_dir.join(&name)).with_context(|| format!("moving {}", from.display()))?;
    Ok(join_path(to_parent, &name))
}

/// Moves the folder into a bin and remembers where it came from. Returns the name it
/// has there, which is the folder name unless that was already taken.
pub fn stash(path: &str, bin: Bin, now: chrono::DateTime<chrono::Local>) -> Result<String> {
    if path.is_empty() {
        bail!("refusing to {} the root", bin.word());
    }
    let from = dir_of(path);
    let store = bin.dir();
    fs::create_dir_all(&store).with_context(|| format!("creating {}", store.display()))?;
    let name = free_name(&store, &from.file_name().unwrap_or_default().to_string_lossy());
    update_meta(&from, |m| {
        m.from = Some(parent_path(path).to_string());
        m.archived = Some(now);
        m.order = None;
    })?;
    fs::rename(&from, store.join(&name))
        .with_context(|| format!("moving {} to the {}", from.display(), bin.word()))?;
    Ok(name)
}

/// Everything in a bin, newest first.
pub fn read_bin(bin: Bin) -> Vec<Thread> {
    let store = bin.dir();
    let Ok(entries) = fs::read_dir(&store) else { return Vec::new() };
    let mut out: Vec<Thread> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            read_node(&e.path(), &name)
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.meta.archived));
    out
}

/// Which bin holds a thread of this name, the archive first.
pub fn bin_of(name: &str) -> Option<Bin> {
    [Bin::Archive, Bin::Trash].into_iter().find(|b| b.dir().join(name).is_dir())
}

fn binned(bin: Bin, name: &str) -> Result<PathBuf> {
    let dir = bin.dir().join(name);
    if !dir.is_dir() {
        bail!("nothing called \"{name}\" in the {}", bin.word());
    }
    Ok(dir)
}

/// Put a thread back where it came from, or at the top if that is gone.
pub fn restore(bin: Bin, name: &str) -> Result<String> {
    let from = binned(bin, name)?;
    let file = read_thread_file(&from);
    let came_from = file.meta.from.clone().unwrap_or_default();
    let parent = if came_from.is_empty() || dir_of(&came_from).is_dir() { came_from.clone() } else { String::new() };
    let parent_dir = dir_of(&parent);
    fs::create_dir_all(&parent_dir)?;
    let to_name = free_name(&parent_dir, came_from.rsplit('/').next().unwrap_or(name));
    let to_name = if to_name.starts_with("thread") { free_name(&parent_dir, &file.title) } else { to_name };
    update_meta(&from, |m| {
        m.from = None;
        m.archived = None;
    })?;
    fs::rename(&from, parent_dir.join(&to_name)).with_context(|| format!("restoring {name}"))?;
    Ok(join_path(&parent, &to_name))
}

/// Really gone. Only a bin can be purged from.
pub fn purge(bin: Bin, name: &str) -> Result<()> {
    let dir = binned(bin, name)?;
    fs::remove_dir_all(&dir).with_context(|| format!("deleting {}", dir.display()))?;
    Ok(())
}

const CLAUDE_MD: &str = r#"# t0 threads

Every folder here is a thread of work: an open loop with a person or a problem. Sub-folders are sub-threads. `thread.md` in each folder is the log: a title line, then one event per line (origin, action, wait, done), then free notes. `chats/` holds the Claude Code sessions started from that thread.

You are usually running inside one thread's folder. Read its `thread.md` first, and the parent folders' if the context matters. Append what you learn to `thread.md` below the log. To add events, use the CLI so the format stays right:

```
t0 status .                          this thread and what is under it
t0 ".: mailed the vendor, wait 2d"   an event on this thread, and start waiting
t0 "new ./<title>: <problem>"        a sub-thread of this one
t0 help
```
"#;

/// First run: make the root and drop a CLAUDE.md so chats know where they are.
pub fn ensure_root() -> Result<()> {
    let root = root();
    fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    let claude = root.join("CLAUDE.md");
    if !claude.exists() {
        fs::write(&claude, CLAUDE_MD).with_context(|| format!("writing {}", claude.display()))?;
    }
    Ok(())
}

pub fn remove_chat(path: &str, file: &str) -> Result<()> {
    let target = dir_of(path).join(CHATS_DIR).join(file);
    if !target.exists() {
        bail!("no chat \"{file}\" in {path}");
    }
    fs::remove_file(target)?;
    Ok(())
}

/// Give every child of `parent` an explicit position, so a hand-placed thread stays
/// put. Returns the ordered folder names.
pub fn number_siblings(parent: &str) -> Result<Vec<String>> {
    let siblings = read_node(&dir_of(parent), parent).children;
    for (i, t) in siblings.iter().enumerate() {
        let want = Some(i as u32 + 1);
        if t.meta.order != want {
            update_meta(&dir_of(&t.path), |m| m.order = want)?;
        }
    }
    Ok(siblings.into_iter().map(|t| t.name).collect())
}

/// Swap a thread with the sibling above or below it.
pub fn reorder(path: &str, delta: i32) -> Result<bool> {
    let parent = parent_path(path).to_string();
    let names = number_siblings(&parent)?;
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some(at) = names.iter().position(|n| n == name) else { return Ok(false) };
    let to = at as i32 + delta;
    if to < 0 || to as usize >= names.len() {
        return Ok(false);
    }
    for (n, order) in [(name, to as u32 + 1), (names[to as usize].as_str(), at as u32 + 1)] {
        update_meta(&dir_of(&join_path(&parent, n)), |m| m.order = Some(order))?;
    }
    Ok(true)
}
