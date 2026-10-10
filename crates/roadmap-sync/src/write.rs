use std::path::Path;

use serde::Serialize;

use crate::error::{Error, Result};

pub fn write_markdown_file<T: Serialize>(path: &Path, frontmatter: &T, body: &str) -> Result<()> {
    let yaml = serde_yaml::to_string(frontmatter).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        message: format!("failed to serialize frontmatter: {e}"),
    })?;

    let content = format!("---\n{yaml}---\n\n{body}\n");
    std::fs::write(path, content).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        message: format!("failed to write file: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::model::{IssueFrontmatter, IssueState};

    #[test]
    fn round_trip_preserves_body() {
        let frontmatter = IssueFrontmatter {
            id: "app-test".into(),
            repo: "app".into(),
            title: "Test".into(),
            url: Some("https://github.com/example/app/issues/1".into()),
            state: IssueState::Open,
            labels: vec!["roadmap".into()],
            milestone: None,
            blocked_by: vec![],
            fields: BTreeMap::from([
                ("status".into(), Some("Backlog".into())),
                ("start_date".into(), Some("2026-07-01".into())),
                ("target_date".into(), Some("2026-07-15".into())),
            ]),
        };
        let body = "## Context\n\nSome text.\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.md");

        write_markdown_file(&path, &frontmatter, body).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.starts_with("---\n"));
        assert!(raw.contains("start_date: 2026-07-01"));
        assert!(raw.contains("target_date: 2026-07-15"));
        assert!(raw.contains("Some text."));
    }
}
