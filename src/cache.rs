//! Remembering what a slow subprocess said, until the thing it described changes.
//!
//! Two providers have to shell out to a toolchain that takes a third of a
//! second to start and then reports something that only changes when files on
//! disk change. A stamp over those files decides whether the cached answer is
//! still good, so the usual scan never starts the process at all.
//!
//! A stamp is never a guess about time passing: an empty stamp, an unreadable
//! cache and a mismatch all mean the same thing, which is to go and ask.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::provider::Installed;

/// What a directory tree looks like right now: one line per file, with its
/// size and modification time, sorted so the order of `read_dir` cannot make
/// two identical directories look different.
pub fn stamp(paths: &[PathBuf]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for path in paths {
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        lines.push(format!("{} {} {modified}", path.display(), meta.len()));
    }
    lines.sort();
    lines.join("\n")
}

/// Every entry of a directory, for stamping.
pub fn entries(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten().map(|entry| entry.path()).collect()
}

#[derive(Serialize, Deserialize)]
struct Cached {
    stamp: String,
    items: Vec<Item>,
}

#[derive(Serialize, Deserialize)]
struct Item {
    key: String,
    label: Option<String>,
    version: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    binaries: Vec<String>,
    app: Option<String>,
}

fn path(name: &str) -> Option<PathBuf> {
    Some(crate::config::Paths::discover().ok()?.cache(name))
}

pub fn load(name: &str, stamp: &str) -> Option<Vec<Installed>> {
    if stamp.is_empty() {
        return None;
    }
    let cached: Cached = serde_json::from_str(&std::fs::read_to_string(path(name)?).ok()?).ok()?;
    if cached.stamp != stamp {
        return None;
    }
    Some(
        cached
            .items
            .into_iter()
            .map(|i| {
                let mut item = Installed::new(i.key);
                item.label = i.label;
                item.version = i.version;
                item.aliases = i.aliases;
                item.binaries = i.binaries;
                item.app = i.app;
                item
            })
            .collect(),
    )
}

/// A cache that cannot be written is not an error: the next scan asks again.
pub fn store(name: &str, stamp: &str, items: &[Installed]) {
    if stamp.is_empty() {
        return;
    }
    let Some(path) = path(name) else { return };
    let cached = Cached {
        stamp: stamp.to_string(),
        items: items
            .iter()
            .map(|i| Item {
                key: i.key.clone(),
                label: i.label.clone(),
                version: i.version.clone(),
                aliases: i.aliases.clone(),
                binaries: i.binaries.clone(),
                app: i.app.clone(),
            })
            .collect(),
    };
    if let Ok(body) = serde_json::to_string(&cached) {
        let _ = std::fs::write(path, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_directory_stamps_to_nothing() {
        assert_eq!(stamp(&entries(Path::new("/no/such/place"))), "");
    }

    #[test]
    fn the_stamp_does_not_depend_on_read_order() {
        let dir = std::env::temp_dir().join(format!("pkginbox-stamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["b", "a", "c"] {
            std::fs::write(dir.join(name), name).unwrap();
        }
        let once = stamp(&entries(&dir));
        let twice = stamp(&entries(&dir));
        assert_eq!(once, twice);
        assert_eq!(once.lines().count(), 3);
    }

    #[test]
    fn an_empty_stamp_is_never_a_hit() {
        assert!(load("whatever", "").is_none());
    }
}
