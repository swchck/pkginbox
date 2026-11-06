use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::util::now;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Seen outside the manifest, no decision yet.
    Pending,
    /// Protected from `brew bundle cleanup` until `expires_at`.
    Quarantined,
    /// Written into the manifest; the record is closed.
    Adopted,
    /// Deliberately left unmanaged; never surfaced again.
    Dismissed,
    /// Uninstalled through pkginbox.
    Removed,
    /// Declared in the manifest but missing from the system.
    Drifted,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Quarantined => "quarantined",
            Status::Adopted => "adopted",
            Status::Dismissed => "dismissed",
            Status::Removed => "removed",
            Status::Drifted => "drifted",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "pending" => Status::Pending,
            "quarantined" => Status::Quarantined,
            "adopted" => Status::Adopted,
            "dismissed" => Status::Dismissed,
            "removed" => Status::Removed,
            "drifted" => Status::Drifted,
            other => bail!("unknown status `{other}`"),
        })
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Package {
    pub provider: String,
    pub key: String,
    pub label: Option<String>,
    pub version: Option<String>,
    pub status: Status,
    pub note: Option<String>,
    pub first_seen: i64,
    pub expires_at: Option<i64>,
    /// Last time the package's command was run or its app opened, when a
    /// signal for it exists at all.
    pub last_used: Option<i64>,
}

impl Package {
    /// The name to show: App Store ids and the like are unreadable on their own.
    pub fn display(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.key)
    }

    /// Installed a while ago and not used once since. `Some(0)` is the signal
    /// saying the history was searched and came back empty, which is what
    /// separates this from a package nothing can be said about.
    pub fn never_used(&self, settled_for: i64) -> bool {
        self.last_used == Some(0) && now() - self.first_seen >= settled_for
    }
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let db = Self {
            conn: Connection::open_in_memory()?,
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < 1 {
            self.conn.execute_batch(
                "CREATE TABLE packages (
                     provider    TEXT    NOT NULL,
                     key         TEXT    NOT NULL,
                     label       TEXT,
                     version     TEXT,
                     status      TEXT    NOT NULL,
                     note        TEXT,
                     first_seen  INTEGER NOT NULL,
                     decided_at  INTEGER,
                     expires_at  INTEGER,
                     last_used   INTEGER,
                     PRIMARY KEY (provider, key)
                 );
                 CREATE TABLE observations (
                     id          INTEGER PRIMARY KEY,
                     provider    TEXT    NOT NULL,
                     key         TEXT    NOT NULL,
                     version     TEXT,
                     source      TEXT    NOT NULL,
                     raw_command TEXT,
                     observed_at INTEGER NOT NULL
                 );
                 CREATE INDEX observations_pkg ON observations (provider, key);
                 PRAGMA user_version = 1;",
            )?;
        }
        Ok(())
    }

    /// Records an installed package that the manifest does not declare. An
    /// existing decision is preserved — only the volatile columns are refreshed.
    pub fn observe_unmanaged(
        &self,
        provider: &str,
        key: &str,
        label: Option<&str>,
        version: Option<&str>,
    ) -> Result<bool> {
        let known: Option<String> = self
            .conn
            .query_row(
                "SELECT status FROM packages WHERE provider = ?1 AND key = ?2",
                params![provider, key],
                |row| row.get(0),
            )
            .optional()?;

        match known {
            None => {
                self.conn.execute(
                    "INSERT INTO packages (provider, key, label, version, status, first_seen)
                     VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                    params![provider, key, label, version, now()],
                )?;
                self.conn.execute(
                    "INSERT INTO observations (provider, key, version, source, observed_at)
                     VALUES (?1, ?2, ?3, 'scan', ?4)",
                    params![provider, key, version, now()],
                )?;
                Ok(true)
            }
            Some(status) => {
                // A package that drifted out of the manifest comes back as undecided.
                let reopen = status == "drifted";
                self.conn.execute(
                    "UPDATE packages
                        SET label = ?3, version = ?4,
                            status = CASE WHEN ?5 THEN 'pending' ELSE status END
                      WHERE provider = ?1 AND key = ?2",
                    params![provider, key, label, version, reopen],
                )?;
                Ok(false)
            }
        }
    }

    /// Closes out a record once the package is both declared and installed,
    /// which is what adopting it by hand looks like from the outside.
    pub fn mark_managed(&self, provider: &str, key: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE packages
                SET status = 'adopted', decided_at = ?3, expires_at = NULL
              WHERE provider = ?1 AND key = ?2
                AND status IN ('pending', 'quarantined', 'drifted')",
            params![provider, key, now()],
        )?;
        Ok(())
    }

    /// Declared but not installed: the next apply will put it back.
    pub fn mark_drifted(&self, provider: &str, key: &str, label: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO packages (provider, key, label, status, first_seen)
             VALUES (?1, ?2, ?3, 'drifted', ?4)
             ON CONFLICT (provider, key) DO UPDATE
                SET status = CASE WHEN packages.status = 'dismissed'
                                  THEN packages.status ELSE 'drifted' END",
            params![provider, key, label, now()],
        )?;
        Ok(())
    }

    /// Closes open records for packages a scan no longer saw at all — either
    /// uninstalled by hand, or recorded under a key an earlier version spelled
    /// differently.
    pub fn close_absent(&self, provider: &str, touched: &HashSet<String>) -> Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT key FROM packages
              WHERE provider = ?1 AND status IN ('pending', 'quarantined', 'drifted')",
        )?;
        let open: Vec<String> = stmt
            .query_map(params![provider], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;

        let mut closed = 0;
        for key in open {
            if touched.contains(&key) {
                continue;
            }
            self.set_status(provider, &key, Status::Removed)?;
            closed += 1;
        }
        Ok(closed)
    }

    /// Records how a package turned up, for the detail view. Scans do not call
    /// this — only the shell hook, which is the one place the exact invocation
    /// is still known.
    pub fn add_observation(
        &self,
        provider: &str,
        key: &str,
        source: &str,
        raw_command: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO observations (provider, key, source, raw_command, observed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![provider, key, source, raw_command, now()],
        )?;
        Ok(())
    }

    /// The command that installed this package, when the hook saw it happen.
    pub fn last_command(&self, provider: &str, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT raw_command FROM observations
                  WHERE provider = ?1 AND key = ?2 AND raw_command IS NOT NULL
                  ORDER BY observed_at DESC LIMIT 1",
                params![provider, key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_last_used(&self, provider: &str, key: &str, at: Option<i64>) -> Result<()> {
        self.conn.execute(
            "UPDATE packages SET last_used = ?3 WHERE provider = ?1 AND key = ?2",
            params![provider, key, at],
        )?;
        Ok(())
    }

    pub fn set_note(&self, provider: &str, key: &str, note: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE packages SET note = ?3 WHERE provider = ?1 AND key = ?2",
            params![provider, key, note.filter(|n| !n.trim().is_empty())],
        )?;
        Ok(())
    }

    pub fn set_status(&self, provider: &str, key: &str, status: Status) -> Result<()> {
        self.conn.execute(
            "UPDATE packages SET status = ?3, decided_at = ?4 WHERE provider = ?1 AND key = ?2",
            params![provider, key, status.as_str(), now()],
        )?;
        Ok(())
    }

    pub fn quarantine(&self, provider: &str, key: &str, until: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE packages
                SET status = 'quarantined', decided_at = ?3, expires_at = ?4
              WHERE provider = ?1 AND key = ?2",
            params![provider, key, now(), until],
        )?;
        Ok(())
    }

    pub fn by_status(&self, status: Status) -> Result<Vec<Package>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider, key, label, version, status, note, first_seen, expires_at, last_used
               FROM packages WHERE status = ?1 ORDER BY provider, key",
        )?;
        let rows = stmt.query_map(params![status.as_str()], row_to_package)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()
    }

    pub fn find(&self, provider: &str, key: &str) -> Result<Option<Package>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider, key, label, version, status, note, first_seen, expires_at, last_used
               FROM packages WHERE provider = ?1 AND key = ?2",
        )?;
        let found = stmt
            .query_map(params![provider, key], row_to_package)?
            .next()
            .transpose()?;
        found.transpose()
    }

    pub fn all(&self) -> Result<Vec<Package>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider, key, label, version, status, note, first_seen, expires_at, last_used
               FROM packages ORDER BY provider, key",
        )?;
        let rows = stmt.query_map([], row_to_package)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()
    }
}

type RowResult = Result<Package>;

fn row_to_package(row: &rusqlite::Row<'_>) -> rusqlite::Result<RowResult> {
    let status: String = row.get(4)?;
    Ok((|| {
        Ok(Package {
            provider: row.get(0)?,
            key: row.get(1)?,
            label: row.get(2)?,
            version: row.get(3)?,
            status: Status::parse(&status)?,
            note: row.get(5)?,
            first_seen: row.get(6)?,
            expires_at: row.get(7)?,
            last_used: row.get(8)?,
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sighting_is_pending_and_repeat_sightings_do_not_reopen_it() {
        let db = Db::open_in_memory().unwrap();
        assert!(
            db.observe_unmanaged("brew", "lazydocker", None, Some("0.24"))
                .unwrap()
        );
        db.set_status("brew", "lazydocker", Status::Dismissed)
            .unwrap();
        assert!(
            !db.observe_unmanaged("brew", "lazydocker", None, Some("0.25"))
                .unwrap()
        );

        let pkg = &db.by_status(Status::Dismissed).unwrap()[0];
        assert_eq!(pkg.version.as_deref(), Some("0.25"));
    }

    #[test]
    fn a_declared_and_installed_package_closes_an_open_record() {
        let db = Db::open_in_memory().unwrap();
        db.observe_unmanaged("cask", "cursor", None, None).unwrap();
        db.mark_managed("cask", "cursor").unwrap();
        assert_eq!(db.by_status(Status::Adopted).unwrap().len(), 1);
    }

    #[test]
    fn mark_managed_leaves_a_dismissed_record_alone() {
        let db = Db::open_in_memory().unwrap();
        db.observe_unmanaged("cask", "arc", None, None).unwrap();
        db.set_status("cask", "arc", Status::Dismissed).unwrap();
        db.mark_managed("cask", "arc").unwrap();
        assert_eq!(db.by_status(Status::Dismissed).unwrap().len(), 1);
    }

    #[test]
    fn drift_reopens_as_pending_once_the_declaration_is_gone() {
        let db = Db::open_in_memory().unwrap();
        db.mark_drifted("brew", "ripgrep", None).unwrap();
        assert!(!db.observe_unmanaged("brew", "ripgrep", None, None).unwrap());
        assert_eq!(db.by_status(Status::Pending).unwrap().len(), 1);
    }

    #[test]
    fn never_used_needs_both_a_searched_history_and_some_age() {
        let db = Db::open_in_memory().unwrap();
        db.observe_unmanaged("brew", "vgmstream", None, None)
            .unwrap();
        db.set_last_used("brew", "vgmstream", Some(0)).unwrap();

        let settled = 86_400 * 14;
        let fresh = &db.by_status(Status::Pending).unwrap()[0];
        assert!(!fresh.never_used(settled), "just installed is not unused");

        db.conn
            .execute(
                "UPDATE packages SET first_seen = ?1",
                params![now() - 86_400 * 30],
            )
            .unwrap();
        let old = &db.by_status(Status::Pending).unwrap()[0];
        assert!(old.never_used(settled));
    }

    #[test]
    fn a_package_with_no_usage_signal_is_never_called_unused() {
        let db = Db::open_in_memory().unwrap();
        db.observe_unmanaged("tap", "someone/tap", None, None)
            .unwrap();
        db.conn
            .execute(
                "UPDATE packages SET first_seen = ?1",
                params![now() - 86_400 * 90],
            )
            .unwrap();
        let pkg = &db.by_status(Status::Pending).unwrap()[0];
        assert_eq!(pkg.last_used, None);
        assert!(!pkg.never_used(86_400 * 14));
    }

    #[test]
    fn quarantine_records_an_expiry() {
        let db = Db::open_in_memory().unwrap();
        db.observe_unmanaged("brew", "oq", None, None).unwrap();
        db.quarantine("brew", "oq", 1_000_000).unwrap();
        let pkg = &db.by_status(Status::Quarantined).unwrap()[0];
        assert_eq!(pkg.expires_at, Some(1_000_000));
    }
}
