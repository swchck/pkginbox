use anyhow::{Result, bail};

const SUFFIX: &str = ">/dev/null 2>&1 || true";
const MIN_GAP: usize = 1;

/// Adds the `brew tap` and `brew trust --tap` lines a newly adopted tap needs.
///
/// Declaring a tap in the manifest is not enough on its own: `brew bundle`
/// refuses formulae from an untrusted tap, so the install script has to tap and
/// trust it before the Brewfile is handed over.
pub fn add_tap(source: &str, profile: &str, tap: &str) -> Result<String> {
    let lines: Vec<&str> = source.lines().collect();
    let (start, end) = trust_region(&lines)?;
    let (block_start, block_end) = profile_block(&lines, start, end, profile)?;

    if lines[block_start..block_end]
        .iter()
        .any(|l| l.trim_start().starts_with(&format!("brew tap {tap} ")))
    {
        bail!("`{tap}` is already tapped in the install script");
    }

    let block = &lines[block_start..block_end];
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();

    // Insert the trust line first: it sits after the tap lines, so adding it
    // first keeps the tap line's index valid.
    let trust = render(
        &format!("brew trust --tap {tap}"),
        suffix_column(block, "brew trust "),
    );
    let at = last_prefixed(block, "brew trust --tap ")
        .map(|i| block_start + i + 1)
        .unwrap_or(block_end);
    out.insert(at, trust);

    let tap_line = render(
        &format!("brew tap {tap}"),
        suffix_column(block, "brew tap "),
    );
    let at = last_prefixed(block, "brew tap ")
        .map(|i| block_start + i + 1)
        .unwrap_or(block_start);
    out.insert(at, tap_line);

    let mut joined = out.join("\n");
    if source.ends_with('\n') {
        joined.push('\n');
    }
    Ok(joined)
}

/// Drops the `brew tap` and `brew trust --tap` lines for a tap that is no
/// longer declared.
pub fn remove_tap(source: &str, tap: &str) -> Result<String> {
    let kept: Vec<&str> = source.lines().filter(|l| !names_tap(l, tap)).collect();
    if kept.len() == source.lines().count() {
        bail!("`{tap}` is not tapped in the install script");
    }
    let mut out = kept.join("\n");
    if source.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

fn names_tap(line: &str, tap: &str) -> bool {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    match tokens.as_slice() {
        ["brew", "tap", name, ..] => *name == tap,
        ["brew", "trust", "--tap", name, ..] => *name == tap,
        _ => false,
    }
}

fn render(command: &str, column: Option<usize>) -> String {
    let target = column.unwrap_or(command.len() + MIN_GAP);
    let pad = target.saturating_sub(command.len()).max(MIN_GAP);
    format!("{command}{}{SUFFIX}", " ".repeat(pad))
}

fn last_prefixed(lines: &[&str], prefix: &str) -> Option<usize> {
    lines.iter().rposition(|l| l.starts_with(prefix))
}

/// The column `>/dev/null …` sits in for one kind of command. Tap and trust
/// lines are aligned independently in this script, so they have to be measured
/// that way too.
fn suffix_column(lines: &[&str], prefix: &str) -> Option<usize> {
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for line in lines.iter().filter(|l| l.starts_with(prefix)) {
        let Some(col) = line.find(SUFFIX) else {
            continue;
        };
        match counts.iter_mut().find(|(c, _)| *c == col) {
            Some((_, n)) => *n += 1,
            None => counts.push((col, 1)),
        }
    }
    counts.sort_by_key(|(col, n)| (std::cmp::Reverse(*n), *col));
    counts.first().map(|(col, _)| *col)
}

/// The stretch of the script that taps and trusts third-party taps.
fn trust_region(lines: &[&str]) -> Result<(usize, usize)> {
    let Some(start) = lines.iter().position(|l| l.starts_with("brew tap ")) else {
        bail!("the install script has no `brew tap` lines to extend");
    };
    let Some(end) = lines.iter().rposition(|l| l.starts_with("brew trust ")) else {
        bail!("the install script has no `brew trust` lines to extend");
    };
    Ok((start, end + 1))
}

/// The slice of the trust region guarded by this machine profile.
fn profile_block(
    lines: &[&str],
    start: usize,
    end: usize,
    profile: &str,
) -> Result<(usize, usize)> {
    if profile == "common" {
        let first_guard = lines[start..end]
            .iter()
            .position(|l| l.trim_start().starts_with("{{ if eq .machine_type"))
            .map(|i| start + i)
            .unwrap_or(end);
        return Ok((start, first_guard));
    }

    let guard = format!("{{{{ if eq .machine_type \"{profile}\" -}}}}");
    let Some(open) = lines[start..]
        .iter()
        .position(|l| l.trim() == guard)
        .map(|i| start + i)
    else {
        bail!("the install script has no `{profile}` branch in its tap section");
    };
    // The closing tag sits past the last trust line, so the search for it runs
    // to the end of the file rather than to the end of the region.
    let Some(close) = lines[open + 1..]
        .iter()
        .position(|l| l.trim().starts_with("{{ end"))
        .map(|i| open + 1 + i)
    else {
        bail!("the `{profile}` branch in the tap section is not closed");
    };
    Ok((open + 1, close))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"#!/usr/bin/env bash

brew tap gabrie30/utils           >/dev/null 2>&1 || true
brew tap abue-ammar/tinycast      >/dev/null 2>&1 || true
brew trust --tap gabrie30/utils           >/dev/null 2>&1 || true
brew trust --tap abue-ammar/tinycast      >/dev/null 2>&1 || true
{{ if eq .machine_type "work" -}}
brew tap derailed/k9s            >/dev/null 2>&1 || true
brew trust --tap derailed/k9s            >/dev/null 2>&1 || true
{{ end }}
# Install packages via Brewfile
"#;

    #[test]
    fn adds_both_lines_to_the_common_block() {
        let out = add_tap(SAMPLE, "common", "swchck/local").unwrap();
        let lines: Vec<&str> = out.lines().collect();
        let tap = lines
            .iter()
            .position(|l| l.starts_with("brew tap swchck/local"))
            .unwrap();
        let trust = lines
            .iter()
            .position(|l| l.starts_with("brew trust --tap swchck/local"))
            .unwrap();
        assert!(lines[tap - 1].contains("abue-ammar/tinycast"));
        assert!(lines[trust - 1].contains("trust --tap abue-ammar/tinycast"));
        assert!(tap < trust);
    }

    #[test]
    fn stays_inside_the_profile_branch() {
        let out = add_tap(SAMPLE, "work", "hashicorp/tap").unwrap();
        let lines: Vec<&str> = out.lines().collect();
        let guard = lines
            .iter()
            .position(|l| l.contains("if eq .machine_type"))
            .unwrap();
        let close = lines
            .iter()
            .position(|l| l.trim().starts_with("{{ end"))
            .unwrap();
        let tap = lines
            .iter()
            .position(|l| l.starts_with("brew tap hashicorp/tap"))
            .unwrap();
        assert!(guard < tap && tap < close, "{:?}", lines);
    }

    #[test]
    fn follows_the_column_of_the_surrounding_block() {
        let out = add_tap(SAMPLE, "common", "swchck/local").unwrap();
        let new = out
            .lines()
            .find(|l| l.starts_with("brew tap swchck/local"))
            .unwrap();
        let reference = out
            .lines()
            .find(|l| l.starts_with("brew tap gabrie30"))
            .unwrap();
        assert_eq!(new.find(SUFFIX), reference.find(SUFFIX));

        let new = out
            .lines()
            .find(|l| l.starts_with("brew trust --tap swchck/local"))
            .unwrap();
        let reference = out
            .lines()
            .find(|l| l.starts_with("brew trust --tap gabrie30"))
            .unwrap();
        assert_eq!(new.find(SUFFIX), reference.find(SUFFIX));
    }

    #[test]
    fn removes_both_lines_of_a_tap() {
        let out = remove_tap(SAMPLE, "derailed/k9s").unwrap();
        assert!(!out.contains("derailed/k9s"));
        assert!(out.contains("gabrie30/utils"));
        assert!(out.contains("{{ if eq .machine_type \"work\" -}}"));
    }

    #[test]
    fn reports_a_tap_that_is_not_in_the_script() {
        let err = remove_tap(SAMPLE, "nobody/nothing").unwrap_err();
        assert!(err.to_string().contains("is not tapped"), "{err}");
    }

    #[test]
    fn refuses_a_tap_that_is_already_there() {
        let err = add_tap(SAMPLE, "work", "derailed/k9s").unwrap_err();
        assert!(err.to_string().contains("already tapped"), "{err}");
    }

    #[test]
    fn reports_a_profile_the_script_does_not_branch_on() {
        let err = add_tap(SAMPLE, "personal", "bahaaio/pomo").unwrap_err();
        assert!(err.to_string().contains("no `personal` branch"), "{err}");
    }
}
