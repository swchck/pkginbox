use anyhow::Result;

use super::{Installed, Provider};
use crate::cache;
use crate::chezmoi::ChezmoiData;
use crate::util::{capture, has_bin, home};

pub struct Mas;

impl Provider for Mas {
    fn id(&self) -> &'static str {
        "mas"
    }

    fn available(&self) -> bool {
        has_bin("mas")
    }

    // Matching is by App Store id: the manifest keys are display names, and
    // those change with app updates and the system language.
    //
    // `mas list` costs a sixth of a second and can only change when an app
    // bundle does, so it is asked only when one has — an install, a removal or
    // an update all rewrite the bundle, and the stamp covers every bundle
    // rather than only the App Store ones, which errs towards asking again.
    fn installed(&self) -> Result<Vec<Installed>> {
        let stamp = cache::stamp(&bundles());
        if let Some(hit) = cache::load("mas", &stamp) {
            return Ok(hit);
        }
        let raw = capture("mas", &["list"])?;
        let items: Vec<Installed> = raw.lines().filter_map(parse_line).collect();
        cache::store("mas", &stamp, &items);
        Ok(items)
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        data.package_table("appstore_apps").into_values().collect()
    }
}

/// Every application bundle the App Store could have put somewhere.
fn bundles() -> Vec<std::path::PathBuf> {
    let mut out = cache::entries(std::path::Path::new(crate::usage::APPLICATIONS));
    if let Ok(home) = home() {
        out.extend(cache::entries(&home.join("Applications")));
    }
    out.retain(|p| p.extension().is_some_and(|e| e == "app"));
    out
}

/// `mas list` prints "<id>  <name>  (<version>)".
fn parse_line(line: &str) -> Option<Installed> {
    let line = line.trim();
    let (id, rest) = line.split_once(char::is_whitespace)?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let rest = rest.trim();
    let (name, version) = match rest.rfind('(') {
        Some(idx) => (
            rest[..idx].trim(),
            Some(rest[idx + 1..].trim_end_matches(')').trim()),
        ),
        None => (rest, None),
    };
    let mut installed = Installed::new(id).with_label(name);
    if let Some(v) = version {
        installed = installed.with_version(v);
    }
    Some(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_mas_line() {
        let got = parse_line("1352778147  Bitwarden  (2024.4.1)").unwrap();
        assert_eq!(got.key, "1352778147");
        assert_eq!(got.label.as_deref(), Some("Bitwarden"));
        assert_eq!(got.version.as_deref(), Some("2024.4.1"));
    }

    #[test]
    fn ignores_a_header_line() {
        assert!(parse_line("No installed apps found").is_none());
    }
}
