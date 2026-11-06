use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::db::{Db, Status};

/// Writes the Brewfile fragment that keeps quarantined packages alive.
///
/// It is consumed only by `brew bundle cleanup`, never by `brew bundle` itself:
/// a quarantined package is deliberately not declared, and a fresh machine must
/// not start installing someone's half-finished experiments.
pub fn write(db: &Db, path: &Path) -> Result<usize> {
    let packages = db.by_status(Status::Quarantined)?;
    let mut lines = Vec::new();
    for pkg in &packages {
        match pkg.provider.as_str() {
            "brew" => lines.push(format!("brew \"{}\"", pkg.key)),
            "cask" => lines.push(format!("cask \"{}\"", pkg.key)),
            "tap" => lines.push(format!("tap \"{}\"", pkg.key)),
            _ => {}
        }
    }

    let body = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, body)?;
    Ok(lines.len())
}
