use std::fmt::Write as _;

use crate::db::Package;
use crate::util::now;

/// How many package names the summary line names before it gives up and counts.
const SAMPLE: usize = 3;
/// How long a package has to have been around before never having been used
/// says anything. Something installed this morning has not been used yet.
pub const SETTLED: i64 = 86_400 * 14;

/// One line for the tail of `chezmoi apply`. Silence when there is nothing to
/// decide — an apply that prints a reminder every time stops being read.
///
/// Quarantined packages are deliberately not counted as undecided: putting one
/// in quarantine *is* the decision, and it would otherwise nag until it expires.
pub fn status_line(
    pending: &[Package],
    drifted: &[Package],
    quarantined: &[Package],
) -> Option<String> {
    let expiring: Vec<&Package> = quarantined
        .iter()
        .filter(|p| p.expires_at.is_some_and(|at| at <= now() + 86_400 * 3))
        .collect();

    if pending.is_empty() && drifted.is_empty() && expiring.is_empty() {
        return None;
    }

    let mut line = String::from("pkginbox:");
    if !pending.is_empty() {
        let _ = write!(
            line,
            " {} outside the manifest ({})",
            pending.len(),
            sample(pending)
        );
    }
    if !drifted.is_empty() {
        let _ = write!(
            line,
            " · {} declared but missing ({})",
            drifted.len(),
            sample(drifted)
        );
    }
    if !expiring.is_empty() {
        let _ = write!(line, " · {} quarantine expiring", expiring.len());
    }
    let unused = pending.iter().filter(|p| p.never_used(SETTLED)).count();
    if unused > 0 {
        let _ = write!(line, " · {unused} never used");
    }
    let _ = write!(line, "\n  run `pkginbox` to triage");
    Some(line)
}

fn sample(packages: &[Package]) -> String {
    let shown: Vec<&str> = packages.iter().take(SAMPLE).map(Package::display).collect();
    let mut out = shown.join(", ");
    if packages.len() > shown.len() {
        let _ = write!(out, ", +{}", packages.len() - shown.len());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Status;

    fn pkg(key: &str) -> Package {
        Package {
            provider: "brew".into(),
            key: key.into(),
            label: None,
            version: None,
            status: Status::Pending,
            note: None,
            first_seen: 0,
            expires_at: None,
            last_used: None,
        }
    }

    #[test]
    fn nothing_to_decide_prints_nothing() {
        assert!(status_line(&[], &[], &[]).is_none());
    }

    #[test]
    fn a_fresh_quarantine_is_not_a_reminder() {
        let mut held = pkg("vgmstream");
        held.status = Status::Quarantined;
        held.expires_at = Some(now() + 86_400 * 30);
        assert!(status_line(&[], &[], &[held]).is_none());
    }

    #[test]
    fn long_lists_are_cut_down_to_a_count() {
        let pending: Vec<Package> = ["a", "b", "c", "d", "e"].iter().map(|k| pkg(k)).collect();
        let line = status_line(&pending, &[], &[]).unwrap();
        assert!(
            line.contains("5 outside the manifest (a, b, c, +2)"),
            "{line}"
        );
    }
}
