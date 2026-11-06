//! Whether a package is actually being used.
//!
//! Neither the manifest nor the inbox can tell a tool you reach for daily from
//! one you installed once and forgot. Two signals can: shell history for
//! commands, and Spotlight's last-used date for applications.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::util::{capture, home};

/// Last use per command name, in seconds since the epoch.
#[derive(Default)]
pub struct Signals {
    commands: HashMap<String, i64>,
}

impl Signals {
    /// Reads atuin's history in one pass. Every failure is silent and yields no
    /// signal: a missing or locked history database is a reason to say nothing
    /// about usage, not to fail a scan.
    pub fn collect() -> Self {
        Self {
            commands: atuin_history().unwrap_or_default(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// The most recent use of any of these command names.
    pub fn command_last_use(&self, names: &[String]) -> Option<i64> {
        names
            .iter()
            .filter_map(|name| self.commands.get(&name.to_lowercase()))
            .max()
            .copied()
    }
}

fn atuin_history() -> Option<HashMap<String, i64>> {
    let path = home().ok()?.join(".local/share/atuin/history.db");
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;

    // One grouped pass: a query per package would be ~70 scans of the history.
    let mut stmt = conn
        .prepare(
            "SELECT CASE WHEN instr(command, ' ') > 0
                         THEN substr(command, 1, instr(command, ' ') - 1)
                         ELSE command END AS bin,
                    MAX(timestamp) / 1000000000
               FROM history
              WHERE deleted_at IS NULL
              GROUP BY bin",
        )
        .ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .ok()?;

    let mut out: HashMap<String, i64> = HashMap::new();
    for row in rows.flatten() {
        let (command, at) = row;
        // history holds both `fd` and `/opt/homebrew/bin/fd`
        let name = command
            .rsplit('/')
            .next()
            .unwrap_or(&command)
            .to_lowercase();
        if name.is_empty() {
            continue;
        }
        out.entry(name)
            .and_modify(|v| *v = (*v).max(at))
            .or_insert(at);
    }
    Some(out)
}

/// Spotlight's last-used date for an application bundle.
pub fn app_last_use(bundle: &str) -> Option<i64> {
    let path = Path::new("/Applications").join(bundle);
    let raw = capture(
        "mdls",
        &[
            "-raw",
            "-name",
            "kMDItemLastUsedDate",
            &path.to_string_lossy(),
        ],
    )
    .ok()?;
    parse_mdls_date(raw.trim())
}

/// `mdls` prints `2026-11-06 10:23:22 +0000`, or `(null)` for an app that has
/// never been opened.
fn parse_mdls_date(raw: &str) -> Option<i64> {
    let (date, rest) = raw.split_once(' ')?;
    let time = rest.split(' ').next()?;

    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;

    let mut time = time.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next()?.parse().ok()?;

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Days between 1970-01-01 and the given date, by Howard Hinnant's algorithm.
/// mdls reports UTC, so no zone handling is needed beyond this.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_mdls_timestamp() {
        assert_eq!(
            parse_mdls_date("2026-11-06 10:23:22 +0000"),
            Some(1_793_960_602)
        );
    }

    #[test]
    fn an_app_that_was_never_opened_has_no_date() {
        assert_eq!(parse_mdls_date("(null)"), None);
    }

    #[test]
    fn the_epoch_is_where_it_should_be() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
    }
}
