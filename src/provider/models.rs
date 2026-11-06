use anyhow::Result;

use super::{Installed, Provider};
use crate::chezmoi::ChezmoiData;
use crate::util::{capture, has_bin};

pub struct Ollama;

impl Provider for Ollama {
    fn id(&self) -> &'static str {
        "ollama"
    }

    fn available(&self) -> bool {
        has_bin("ollama")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        // A stopped server makes this fail rather than return nothing, and a
        // scan should not die because ollama happens not to be running.
        let Ok(raw) = capture("ollama", &["list"]) else {
            return Ok(Vec::new());
        };
        Ok(parse(&raw))
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        data.package_path_list(&["ollama", "models"])
    }
}

/// `ollama list` is a table whose first column is `model:tag`; the manifest
/// names models without a tag.
fn parse(raw: &str) -> Vec<Installed> {
    raw.lines()
        .skip(1)
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            let (model, tag) = match name.split_once(':') {
                Some((model, tag)) => (model, Some(tag)),
                None => (name, None),
            };
            let mut item = Installed::new(model).with_alias(name);
            item.version = tag.map(str::to_owned);
            Some(item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_tag_the_manifest_does_not_carry() {
        let got = parse("NAME            ID      SIZE\nllama3.2:latest abc     2 GB\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "llama3.2");
        assert_eq!(got[0].version.as_deref(), Some("latest"));
    }
}
