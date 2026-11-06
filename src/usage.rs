//! Whether a package is actually being used.
//!
//! Neither the manifest nor the inbox can tell a tool you reach for daily from
//! one you installed once and forgot. Two signals can: shell history for
//! commands, and Spotlight's last-used date for applications.

use std::collections::HashMap;
use std::sync::OnceLock;

use rusqlite::{Connection, OpenFlags};

use crate::util::{capture, home};

pub const APPLICATIONS: &str = "/Applications";

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

/// What Spotlight knows about an installed application.
#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppMeta {
    pub bundle_id: Option<String>,
    pub last_used: Option<i64>,
}

/// How long a last-used date may be stale. This cache is the one that ages
/// rather than being stamped against files, because the thing it describes —
/// when you last opened an app — changes without any file changing. An hour
/// is far inside the resolution of the only question asked of it: whether a
/// package has gone untouched for weeks.
const FRESH_FOR: i64 = 3600;

/// Everything in /Applications, read in a single `mdls` call.
///
/// One call rather than two per bundle: asking Spotlight separately for each
/// app's identifier and last-used date was sixty processes. Even batched it is
/// the slowest thing left in a scan, hence the cache.
pub fn applications() -> &'static HashMap<String, AppMeta> {
    static APPS: OnceLock<HashMap<String, AppMeta>> = OnceLock::new();
    APPS.get_or_init(|| {
        if let Some(cached) = load_cache() {
            return cached;
        }
        let apps = read_applications().unwrap_or_default();
        store_cache(&apps);
        apps
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AppCache {
    written_at: i64,
    apps: HashMap<String, AppMeta>,
}

fn cache_path() -> Option<std::path::PathBuf> {
    Some(crate::config::Paths::discover().ok()?.cache("apps"))
}

fn load_cache() -> Option<HashMap<String, AppMeta>> {
    let raw = std::fs::read_to_string(cache_path()?).ok()?;
    let cache: AppCache = serde_json::from_str(&raw).ok()?;
    let age = crate::util::now() - cache.written_at;
    (0..FRESH_FOR).contains(&age).then_some(cache.apps)
}

fn store_cache(apps: &HashMap<String, AppMeta>) {
    let Some(path) = cache_path() else { return };
    let cache = AppCache {
        written_at: crate::util::now(),
        apps: apps.clone(),
    };
    if let Ok(body) = serde_json::to_string(&cache) {
        let _ = std::fs::write(path, body);
    }
}

fn read_applications() -> Option<HashMap<String, AppMeta>> {
    let mut names: Vec<String> = std::fs::read_dir(APPLICATIONS)
        .ok()?
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_str()?.to_string();
            name.ends_with(".app").then_some(name)
        })
        .collect();
    names.sort();
    if names.is_empty() {
        return Some(HashMap::new());
    }

    let paths: Vec<String> = names
        .iter()
        .map(|name| format!("{APPLICATIONS}/{name}"))
        .collect();
    let mut args = vec![
        "-name",
        "kMDItemCFBundleIdentifier",
        "-name",
        "kMDItemLastUsedDate",
    ];
    args.extend(paths.iter().map(String::as_str));
    let raw = capture("mdls", &args).ok()?;

    Some(names.into_iter().zip(parse_mdls(&raw)).collect())
}

/// `mdls` prints the requested attributes for each file in turn, in the order
/// the files were given, always one line per attribute — `(null)` when it has
/// no value — so the blocks line up with the input.
fn parse_mdls(raw: &str) -> Vec<AppMeta> {
    let mut out = Vec::new();
    let mut current = AppMeta::default();
    let mut seen = 0;

    for line in raw.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match name.trim() {
            "kMDItemCFBundleIdentifier" => {
                current.bundle_id =
                    Some(value.trim_matches('"').to_string()).filter(|v| v != "(null)")
            }
            "kMDItemLastUsedDate" => current.last_used = parse_mdls_date(value),
            _ => continue,
        }
        seen += 1;
        if seen == 2 {
            out.push(std::mem::take(&mut current));
            seen = 0;
        }
    }
    out
}

/// Spotlight's last-used date for an application bundle.
pub fn app_last_use(bundle: &str) -> Option<i64> {
    applications().get(bundle)?.last_used
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
    fn splits_mdls_output_per_file() {
        let raw = "kMDItemCFBundleIdentifier = \"com.github.wez.wezterm\"\n\
                   kMDItemLastUsedDate       = 2026-11-06 10:23:22 +0000\n\
                   kMDItemCFBundleIdentifier = \"com.apple.Safari\"\n\
                   kMDItemLastUsedDate       = (null)\n";
        let got = parse_mdls(raw);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].bundle_id.as_deref(), Some("com.github.wez.wezterm"));
        assert_eq!(got[0].last_used, Some(1_793_960_602));
        assert_eq!(got[1].bundle_id.as_deref(), Some("com.apple.Safari"));
        assert_eq!(got[1].last_used, None);
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
