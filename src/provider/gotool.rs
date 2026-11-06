use anyhow::Result;

use super::{Installed, Provider};
use crate::chezmoi::ChezmoiData;
use crate::util::{capture, has_bin};

pub struct Go;

impl Provider for Go {
    fn id(&self) -> &'static str {
        "go"
    }

    fn available(&self) -> bool {
        has_bin("go")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        let Some(bin_dir) = bin_dir() else {
            return Ok(Vec::new());
        };
        let Ok(raw) = capture("go", &["version", "-m", &bin_dir]) else {
            return Ok(Vec::new());
        };
        Ok(parse(&raw))
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        data.profile_list(&["packages", "go"])
    }
}

fn bin_dir() -> Option<String> {
    if let Some(gobin) = std::env::var_os("GOBIN").filter(|v| !v.is_empty()) {
        return Some(gobin.to_string_lossy().into_owned());
    }
    let gopath = capture("go", &["env", "GOPATH"]).ok()?;
    let gopath = gopath.trim();
    (!gopath.is_empty()).then(|| format!("{gopath}/bin"))
}

/// Reads `go version -m <dir>`: one block per binary, `path` naming what was
/// installed and `mod` carrying the module version.
fn parse(raw: &str) -> Vec<Installed> {
    let mut out = Vec::new();
    let mut path: Option<&str> = None;
    let mut version: Option<&str> = None;

    let mut flush = |path: &mut Option<&str>, version: &mut Option<&str>| {
        if let (Some(p), Some(v)) = (path.take(), version.take())
            && !is_local_build(v)
        {
            let binary = p.rsplit('/').next().unwrap_or(p);
            out.push(
                Installed::new(format!("{p}@latest"))
                    .with_version(v)
                    .with_alias(p)
                    .with_alias(format!("{p}@{v}"))
                    .with_binary(binary),
            );
        }
    };

    for line in raw.lines() {
        let Some(fields) = line.strip_prefix('\t') else {
            flush(&mut path, &mut version);
            continue;
        };
        let mut parts = fields.split('\t').filter(|p| !p.is_empty());
        match parts.next() {
            Some("path") => path = parts.next(),
            Some("mod") => {
                let _module = parts.next();
                version = parts.next();
            }
            _ => {}
        }
    }
    flush(&mut path, &mut version);
    out
}

/// A binary built from a working tree rather than installed from a module. The
/// manifest cannot reproduce one, so it is not drift.
fn is_local_build(version: &str) -> bool {
    version == "(devel)" || version.contains("+dirty")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "/bin/oq: go1.26.1\n\tpath\tgithub.com/plutov/oq\n\tmod\tgithub.com/plutov/oq\tv0.3.0\th1:x=\n\tdep\tfoo\tv1\n/bin/example: go1.26.1\n\tpath\tgithub.com/me/proj/example\n\tmod\tgithub.com/me/proj\tv0.0.0-2026+dirty\t\n";

    #[test]
    fn reads_the_installed_path_and_version() {
        let got = parse(SAMPLE);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "github.com/plutov/oq@latest");
        assert_eq!(got[0].version.as_deref(), Some("v0.3.0"));
    }

    #[test]
    fn skips_a_binary_built_from_a_working_tree() {
        assert!(!parse(SAMPLE).iter().any(|i| i.key.contains("example")));
    }

    #[test]
    fn the_bare_path_still_matches_a_manifest_entry() {
        let got = parse(SAMPLE);
        assert!(got[0].identities().any(|n| n == "github.com/plutov/oq"));
    }
}
