//! Turning what someone typed into the thread they meant.

use crate::model::{walk_all, Thread};
use crate::store;
use anyhow::{bail, Result};

/// Where a command was typed from, so "." and "./x" mean something.
#[derive(Clone, Default)]
pub struct Here(pub Option<String>);

impl Here {
    /// The thread whose folder the shell is sitting in, if any.
    pub fn from_cwd() -> Here {
        let root = store::root().canonicalize().ok();
        let cwd = std::env::current_dir().ok().and_then(|p| p.canonicalize().ok());
        let rel = match (root, cwd) {
            (Some(r), Some(c)) => c.strip_prefix(&r).ok().map(|p| p.to_string_lossy().to_string()),
            _ => None,
        };
        Here(rel.filter(|p| !p.is_empty()))
    }

    fn expand(&self, reference: &str) -> Result<String> {
        let r = reference.trim();
        if r != "." && !r.starts_with("./") {
            return Ok(r.to_string());
        }
        let Some(here) = &self.0 else {
            bail!("\".\" means the thread you are inside, and this is not a thread folder");
        };
        Ok(match r.strip_prefix("./") {
            Some(rest) if !rest.is_empty() => format!("{here}/{rest}"),
            _ => here.clone(),
        })
    }
}

fn same_name(t: &Thread, s: &str) -> bool {
    t.name.eq_ignore_ascii_case(s) || t.title.eq_ignore_ascii_case(s)
}

/// Exact path first, then a path whose segments are folder names or titles, then a
/// unique folder name or title anywhere in the tree.
pub fn resolve<'a>(tree: &'a [Thread], reference: &str, here: &Here) -> Result<&'a Thread> {
    let clean = here.expand(reference)?;
    let clean = clean.trim().trim_matches('/');
    if clean.is_empty() {
        bail!("which thread?");
    }
    let all = walk_all(tree);
    if let Some(t) = all.iter().find(|t| t.path == clean) {
        return Ok(t);
    }
    if clean.contains('/') {
        let mut level: &[Thread] = tree;
        let mut found: Option<&Thread> = None;
        for seg in clean.split('/') {
            let seg = seg.trim();
            // an exact folder name wins over a title, so a duplicate title cannot shadow it
            let hit = level
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(seg))
                .or_else(|| level.iter().find(|t| t.title.eq_ignore_ascii_case(seg)));
            match hit {
                Some(t) => {
                    found = Some(t);
                    level = &t.children;
                }
                None => {
                    found = None;
                    break;
                }
            }
        }
        if let Some(t) = found {
            return Ok(t);
        }
    }
    let named: Vec<&&Thread> = all.iter().filter(|t| same_name(t, clean)).collect();
    match named.len() {
        1 => return Ok(named[0]),
        0 => {}
        _ => bail!(
            "\"{reference}\" is ambiguous: {}",
            named.iter().map(|t| t.path.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
    let key = slug_like(clean);
    let near: Vec<&str> = all
        .iter()
        .filter(|t| !key.is_empty() && (t.path.contains(&key) || t.title.to_lowercase().contains(&key)))
        .map(|t| t.path.as_str())
        .take(5)
        .collect();
    if near.is_empty() {
        bail!("no thread \"{reference}\"")
    }
    bail!("no thread \"{reference}\", did you mean: {}", near.join(", "))
}

fn slug_like(s: &str) -> String {
    s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}
