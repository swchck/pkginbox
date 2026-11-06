//! Sharing the inbox between machines.
//!
//! The database stays local — a package is installed on one machine, and so is
//! the decision about it. What travels is a snapshot per machine, written next
//! to the manifest in the dotfiles repository. One writer per file means the
//! snapshots never produce a merge conflict, and the machine is identified by
//! the file name itself.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::util::{capture, now};

const DIR: &str = "inbox";

#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub hostname: String,
    pub machine_type: String,
    pub updated_at: i64,
    pub packages: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
pub struct Entry {
    pub provider: String,
    pub key: String,
    pub status: String,
    pub first_seen: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The repository that holds the manifest. With a `.chezmoiroot` the source
/// directory is a subdirectory of it, and snapshots belong outside the source
/// so that chezmoi never tries to apply them to the home directory.
fn repo_root(source_dir: &Path) -> &Path {
    source_dir.parent().unwrap_or(source_dir)
}

pub fn path_for(source_dir: &Path, hostname: &str) -> PathBuf {
    repo_root(source_dir)
        .join(DIR)
        .join(format!("{hostname}.json"))
}

/// Providers whose entries never leave the machine.
///
/// A snapshot answers "what did the other machine decide about this package",
/// and an application dragged out of a disk image has no manifest entry for
/// any machine to decide about — the README says as much. So the entry would
/// be useless anyway, and it is worse than useless: the dotfiles repository
/// may well be public, and a list of every application on a work laptop is
/// not something to publish by accident.
const LOCAL_ONLY: [&str; 1] = ["app"];

pub fn write(db: &Db, source_dir: &Path, hostname: &str, machine_type: &str) -> Result<PathBuf> {
    let packages = db
        .all()?
        .into_iter()
        .filter(|p| !LOCAL_ONLY.contains(&p.provider.as_str()))
        .map(|p| Entry {
            provider: p.provider,
            key: p.key,
            status: p.status.as_str().to_string(),
            first_seen: p.first_seen,
            note: p.note,
        })
        .collect();

    let snapshot = Snapshot {
        hostname: hostname.to_string(),
        machine_type: machine_type.to_string(),
        updated_at: now(),
        packages,
    };

    let path = path_for(source_dir, hostname);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&snapshot)?),
    )?;
    Ok(path)
}

/// Snapshots written by every machine but this one.
pub fn read_others(source_dir: &Path, hostname: &str) -> Vec<Snapshot> {
    let dir = repo_root(source_dir).join(DIR);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "json" {
                return None;
            }
            let snapshot: Snapshot = serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()?;
            (snapshot.hostname != hostname).then_some(snapshot)
        })
        .collect()
}

/// Which other machines have this package, and what they decided about it.
pub fn elsewhere<'a>(others: &'a [Snapshot], provider: &str, key: &str) -> Vec<(&'a str, &'a str)> {
    others
        .iter()
        .filter_map(|snapshot| {
            let entry = snapshot
                .packages
                .iter()
                .find(|e| e.provider == provider && e.key == key)?;
            Some((snapshot.hostname.as_str(), entry.status.as_str()))
        })
        .collect()
}

pub fn commit(source_dir: &Path, path: &Path) -> Result<bool> {
    let root = repo_root(source_dir);
    let root = root.to_string_lossy();
    let file = path.to_string_lossy();

    capture("git", &["-C", &root, "add", "--", &file]).context("could not stage the snapshot")?;
    if capture(
        "git",
        &["-C", &root, "diff", "--cached", "--quiet", "--", &file],
    )
    .is_ok()
    {
        return Ok(false);
    }
    let name = path.file_stem().unwrap_or_default().to_string_lossy();
    capture(
        "git",
        &[
            "-C",
            &root,
            "commit",
            "-q",
            "-m",
            &format!("chore(inbox): refresh {name}"),
            "--",
            &file,
        ],
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(hostname: &str, key: &str, status: &str) -> Snapshot {
        Snapshot {
            hostname: hostname.into(),
            machine_type: "personal".into(),
            updated_at: 0,
            packages: vec![Entry {
                provider: "brew".into(),
                key: key.into(),
                status: status.into(),
                first_seen: 0,
                note: None,
            }],
        }
    }

    #[test]
    fn applications_stay_on_the_machine_they_are_installed_on() {
        let db = Db::open_in_memory().unwrap();
        db.observe_unmanaged("brew", "mole", None, None).unwrap();
        db.observe_unmanaged("app", "FortiClient.app", Some("FortiClient"), None)
            .unwrap();

        let dir = std::env::temp_dir().join(format!("pkginbox-export-{}", std::process::id()));
        let source = dir.join("src");
        std::fs::create_dir_all(&source).unwrap();
        let path = write(&db, &source, "laptop", "personal").unwrap();

        let written: Snapshot =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let keys: Vec<&str> = written.packages.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(keys, vec!["mole"]);
    }

    #[test]
    fn snapshots_live_beside_the_source_directory_not_inside_it() {
        let path = path_for(Path::new("/repo/src"), "laptop");
        assert_eq!(path, Path::new("/repo/inbox/laptop.json"));
    }

    #[test]
    fn reports_what_other_machines_decided() {
        let others = vec![
            snapshot("laptop", "mole", "adopted"),
            snapshot("mini", "jq", "pending"),
        ];
        assert_eq!(
            elsewhere(&others, "brew", "mole"),
            vec![("laptop", "adopted")]
        );
        assert!(elsewhere(&others, "brew", "nothing").is_empty());
    }
}
