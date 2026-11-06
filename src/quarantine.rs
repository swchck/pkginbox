use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::db::{Db, Status};

/// Every status whose packages must survive `brew bundle cleanup`.
///
/// Undecided is the important one: a package you installed by hand an hour ago
/// has made no promises either way, and deleting it the next time the manifest
/// is applied is exactly the accident this tool exists to prevent. Deliberately
/// left unmanaged counts too — that is a decision to keep the package, just not
/// to declare it.
const PROTECTED: [Status; 3] = [Status::Pending, Status::Quarantined, Status::Dismissed];

/// Writes the Brewfile fragment that keeps those packages alive.
///
/// It is consumed only by `brew bundle cleanup`, never by `brew bundle` itself:
/// none of these packages is declared, and a fresh machine must not start
/// installing someone's half-finished experiments.
pub fn write(db: &Db, path: &Path) -> Result<usize> {
    let mut packages = Vec::new();
    for status in PROTECTED {
        packages.extend(db.by_status(status)?);
    }
    packages.sort_by(|a, b| (&a.provider, &a.key).cmp(&(&b.provider, &b.key)));
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

#[cfg(test)]
mod tests {
    use super::*;

    // one file per test: these run in parallel and a shared path has them
    // overwriting each other's output
    fn brewfile(db: &Db, name: &str) -> String {
        let path =
            std::env::temp_dir().join(format!("pkginbox-q-{}-{name}.brewfile", std::process::id()));
        write(db, &path).unwrap();
        std::fs::read_to_string(&path).unwrap()
    }

    #[test]
    fn an_undecided_package_survives_cleanup() {
        let db = Db::open_in_memory().unwrap();
        db.observe_unmanaged("brew", "mole", None, None).unwrap();
        assert!(brewfile(&db, "undecided").contains("brew \"mole\""));
    }

    #[test]
    fn so_does_one_left_unmanaged_on_purpose() {
        let db = Db::open_in_memory().unwrap();
        db.observe_unmanaged("cask", "arc", None, None).unwrap();
        db.set_status("cask", "arc", Status::Dismissed).unwrap();
        assert!(brewfile(&db, "dismissed").contains("cask \"arc\""));
    }

    #[test]
    fn an_adopted_package_does_not_need_protecting() {
        let db = Db::open_in_memory().unwrap();
        db.observe_unmanaged("brew", "jq", None, None).unwrap();
        db.set_status("brew", "jq", Status::Adopted).unwrap();
        assert_eq!(brewfile(&db, "adopted"), "");
    }

    #[test]
    fn a_missing_package_is_nothing_to_keep_alive() {
        let db = Db::open_in_memory().unwrap();
        db.mark_drifted("brew", "vault", None).unwrap();
        assert_eq!(brewfile(&db, "drifted"), "");
    }
}
