use anyhow::{Result, bail};

/// Indentation every array entry in the manifest uses.
const INDENT: &str = "    ";
/// Minimum gap between an entry and its trailing comment when the array has no
/// column of its own to follow.
const MIN_GAP: usize = 2;

struct Array {
    /// Line holding `key = [`.
    open: usize,
    /// Line holding the closing `]`.
    close: usize,
}

fn find_table(lines: &[&str], table: &str) -> Option<usize> {
    let header = format!("[{table}]");
    lines.iter().position(|l| l.trim() == header)
}

fn find_array(lines: &[&str], table: &str, key: &str) -> Result<Array> {
    let Some(start) = find_table(lines, table) else {
        bail!("no `[{table}]` section in this file");
    };

    let opener = format!("{key} = [");
    let mut open = None;
    for (idx, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') && !trimmed.starts_with("[[") {
            break;
        }
        if trimmed == opener {
            open = Some(idx);
            break;
        }
    }
    let Some(open) = open else {
        bail!("no `{key}` array under `[{table}]`");
    };

    let close = lines
        .iter()
        .enumerate()
        .skip(open + 1)
        .find(|(_, l)| l.trim() == "]")
        .map(|(idx, _)| idx);
    let Some(close) = close else {
        bail!("`{key}` array under `[{table}]` is not closed");
    };

    Ok(Array { open, close })
}

/// Group headings inside an array — the `# Infrastructure` style comments that
/// split it into sections.
pub fn groups(source: &str, table: &str, key: &str) -> Result<Vec<String>> {
    let lines: Vec<&str> = source.lines().collect();
    let array = find_array(&lines, table, key)?;
    Ok(lines[array.open + 1..array.close]
        .iter()
        .map(|l| l.trim())
        .filter(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim().to_string())
        .collect())
}

/// The column the array keeps its trailing comments in.
///
/// The most common column wins rather than the widest entry: these arrays are
/// hand-aligned and a few lines have always been off, so following the majority
/// keeps an insertion down to one changed line instead of reflowing the array.
fn comment_column(lines: &[&str]) -> Option<usize> {
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for line in lines {
        let Some(col) = comment_col_of(line) else {
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

fn comment_col_of(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || !trimmed.starts_with('"') {
        return None;
    }
    let quote_end = trimmed[1..].find('"')? + 1;
    let hash = trimmed[quote_end..].find('#')? + quote_end;
    Some(line.len() - trimmed.len() + hash)
}

fn render_entry(entry: &str, comment: Option<&str>, column: Option<usize>) -> String {
    let value = format!("{INDENT}\"{entry}\",");
    let Some(comment) = comment.map(str::trim).filter(|c| !c.is_empty()) else {
        return value;
    };
    let target = column.unwrap_or(value.len() + MIN_GAP);
    let pad = target.saturating_sub(value.len()).max(MIN_GAP);
    format!("{value}{}# {comment}", " ".repeat(pad))
}

/// Adds a string entry to a manifest array, optionally under one of its group
/// headings. Returns the whole file.
pub fn insert_into_array(
    source: &str,
    table: &str,
    key: &str,
    entry: &str,
    comment: Option<&str>,
    group: Option<&str>,
) -> Result<String> {
    let lines: Vec<&str> = source.lines().collect();
    let array = find_array(&lines, table, key)?;
    let body = &lines[array.open + 1..array.close];

    let quoted = format!("\"{entry}\"");
    if body.iter().any(|l| l.trim_start().starts_with(&quoted)) {
        bail!("`{entry}` is already in `{key}`");
    }

    let at = match group {
        Some(group) => group_end(body, group)
            .map(|offset| array.open + 1 + offset)
            .unwrap_or(array.close),
        None => array.close,
    };

    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    out.insert(at, render_entry(entry, comment, comment_column(body)));
    Ok(finish(out, source))
}

/// The line just past the last entry of a group, where a new member belongs.
fn group_end(body: &[&str], group: &str) -> Option<usize> {
    let start = body
        .iter()
        .position(|l| l.trim().trim_start_matches('#').trim() == group)?;
    let mut end = start + 1;
    while end < body.len() {
        let trimmed = body[end].trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            break;
        }
        end += 1;
    }
    Some(end)
}

/// Adds a `Name = value` line to a manifest table, before the next section.
pub fn insert_into_table(
    source: &str,
    table: &str,
    name: &str,
    value: &str,
    comment: Option<&str>,
) -> Result<String> {
    let lines: Vec<&str> = source.lines().collect();
    let Some(start) = find_table(&lines, table) else {
        bail!("no `[{table}]` section in this file");
    };

    let mut end = lines.len();
    for (idx, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim_start().starts_with('[') {
            end = idx;
            break;
        }
    }
    let body = &lines[start + 1..end];

    // A name with a dot or a space is not a bare TOML key.
    let key = if name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        name.to_string()
    } else {
        format!("\"{name}\"")
    };
    if body
        .iter()
        .any(|l| l.trim_start().starts_with(&format!("{key} ")))
    {
        bail!("`{name}` is already in `{table}`");
    }

    let at = start
        + 1
        + body
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map_or(0, |i| i + 1);
    let assignment = format!("{key} = {value}");
    let line = match comment.map(str::trim).filter(|c| !c.is_empty()) {
        Some(comment) => {
            let column = comment_column_of_table(body).unwrap_or(assignment.len() + MIN_GAP);
            let pad = column.saturating_sub(assignment.len()).max(MIN_GAP);
            format!("{assignment}{}# {comment}", " ".repeat(pad))
        }
        None => assignment,
    };

    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    out.insert(at, line);
    Ok(finish(out, source))
}

fn comment_column_of_table(lines: &[&str]) -> Option<usize> {
    let cols: Vec<&str> = lines
        .iter()
        .filter(|l| !l.trim_start().starts_with('#') && l.contains('='))
        .copied()
        .collect();
    comment_column_raw(&cols)
}

fn comment_column_raw(lines: &[&str]) -> Option<usize> {
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for line in lines {
        let Some(hash) = line.find('#') else { continue };
        match counts.iter_mut().find(|(c, _)| *c == hash) {
            Some((_, n)) => *n += 1,
            None => counts.push((hash, 1)),
        }
    }
    counts.sort_by_key(|(col, n)| (std::cmp::Reverse(*n), *col));
    counts.first().map(|(col, _)| *col)
}

fn finish(lines: Vec<String>, source: &str) -> String {
    let mut out = lines.join("\n");
    if source.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# Common packages

[packages.darwin.common]

taps = [
    "gabrie30/utils",           # Ghorg repository cloner
]

cli = [
    # Git
    "gh",                   # GitHub CLI
    "act",                  # Run GitHub Actions locally

    # Infrastructure
    "kaskade",              # Kafka TUI
]

[packages.darwin.common.appstore_apps]
Telegram = 747648890          # Telegram Messenger
TestFlight = 899247664        # Beta app distribution
"#;

    #[test]
    fn appends_to_the_end_of_an_array() {
        let out = insert_into_array(
            SAMPLE,
            "packages.darwin.common",
            "cli",
            "jq",
            Some("JSON"),
            None,
        )
        .unwrap();
        let cli: Vec<&str> = out
            .lines()
            .skip_while(|l| !l.starts_with("cli = ["))
            .take_while(|l| *l != "]")
            .collect();
        assert_eq!(
            cli.last().unwrap().trim_start(),
            "\"jq\",                   # JSON"
        );
    }

    #[test]
    fn inserts_at_the_end_of_a_named_group() {
        let out = insert_into_array(
            SAMPLE,
            "packages.darwin.common",
            "cli",
            "lazygit",
            Some("Git TUI"),
            Some("Git"),
        )
        .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.contains("\"lazygit\""))
            .unwrap();
        assert!(
            lines[at - 1].contains("\"act\""),
            "{:?}",
            &lines[at - 2..=at]
        );
        assert!(lines[at + 1].trim().is_empty());
    }

    #[test]
    fn follows_the_column_the_array_already_uses() {
        let out = insert_into_array(
            SAMPLE,
            "packages.darwin.common",
            "cli",
            "fd",
            Some("find"),
            None,
        )
        .unwrap();
        let line = out.lines().find(|l| l.contains("\"fd\"")).unwrap();
        let reference = out.lines().find(|l| l.contains("\"gh\"")).unwrap();
        assert_eq!(line.find('#'), reference.find('#'));
    }

    #[test]
    fn a_long_entry_keeps_a_gap_instead_of_reflowing_the_array() {
        let long = "some/very/long/tap-qualified-formula-name";
        let out = insert_into_array(
            SAMPLE,
            "packages.darwin.common",
            "cli",
            long,
            Some("x"),
            None,
        )
        .unwrap();
        let line = out.lines().find(|l| l.contains(long)).unwrap();
        assert!(line.ends_with("\",  # x"), "{line}");
        assert!(out.contains("\"gh\",                   # GitHub CLI"));
    }

    #[test]
    fn refuses_a_duplicate() {
        let err = insert_into_array(SAMPLE, "packages.darwin.common", "cli", "gh", None, None)
            .unwrap_err();
        assert!(err.to_string().contains("already in"), "{err}");
    }

    #[test]
    fn reports_a_missing_array() {
        let err = insert_into_array(SAMPLE, "packages.darwin.common", "casks", "arc", None, None)
            .unwrap_err();
        assert!(err.to_string().contains("no `casks` array"), "{err}");
    }

    #[test]
    fn lists_group_headings() {
        let got = groups(SAMPLE, "packages.darwin.common", "cli").unwrap();
        assert_eq!(got, vec!["Git", "Infrastructure"]);
    }

    #[test]
    fn appends_to_an_app_store_table() {
        let out = insert_into_table(
            SAMPLE,
            "packages.darwin.common.appstore_apps",
            "Bitwarden",
            "1352778147",
            Some("Password manager"),
        )
        .unwrap();
        let line = out.lines().find(|l| l.starts_with("Bitwarden")).unwrap();
        let reference = out.lines().find(|l| l.starts_with("Telegram")).unwrap();
        assert_eq!(line.find('#'), reference.find('#'));
    }

    #[test]
    fn quotes_an_app_name_that_is_not_a_bare_key() {
        let out = insert_into_table(
            SAMPLE,
            "packages.darwin.common.appstore_apps",
            "Apple Configurator",
            "1037126344",
            None,
        )
        .unwrap();
        assert!(out.contains("\"Apple Configurator\" = 1037126344"), "{out}");
    }
}
