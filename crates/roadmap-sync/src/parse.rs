use std::ffi::OsStr;
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalIdKind {
    Temporary(u64),
    GitHub(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalId {
    /// Filename prefix: repo id (issue) or `milestone`.
    pub entity_prefix: String,
    /// Slug segment after the `T<n>` / `#<n>` id (underscores only).
    pub slug: String,
    pub kind: LocalIdKind,
}

impl LocalId {
    pub fn is_temporary(&self) -> bool {
        matches!(self.kind, LocalIdKind::Temporary(_))
    }

    pub fn github_number(&self) -> Option<u64> {
        match self.kind {
            LocalIdKind::GitHub(number) => Some(number),
            LocalIdKind::Temporary(_) => None,
        }
    }
}

pub struct ParsedMarkdown<T> {
    pub local_id: LocalId,
    pub frontmatter: T,
    pub body: String,
}

pub fn parse_markdown_file<T>(path: &Path) -> Result<ParsedMarkdown<T>>
where
    T: DeserializeOwned,
{
    let raw = std::fs::read_to_string(path)
        .map_err(|e| Error::Parse { path: path.display().to_string(), message: e.to_string() })?;
    parse_markdown(&raw, path)
}

pub fn parse_markdown<T>(raw: &str, path: &Path) -> Result<ParsedMarkdown<T>>
where
    T: DeserializeOwned,
{
    let (yaml, body) = split_frontmatter(raw).ok_or_else(|| Error::Parse {
        path: path.display().to_string(),
        message: "missing YAML frontmatter fenced with ---".into(),
    })?;

    let frontmatter: T = serde_yaml::from_str(yaml).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        message: format!("invalid frontmatter YAML: {e}"),
    })?;

    let local_id = parse_filename_id(path)?;

    Ok(ParsedMarkdown {
        local_id,
        frontmatter,
        body: body.trim_start_matches('\n').trim_end().to_string(),
    })
}

fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    let raw = raw.strip_prefix("---\n")?;
    let end = raw.find("\n---\n")?;
    let yaml = &raw[..end];
    let body = &raw[end + 5..];
    Some((yaml, body))
}

/// Parse `<prefix>-<T|#><n>-<slug>` (e.g. `app-T5-implement_feature_a.md`).
pub fn parse_filename_id(path: &Path) -> Result<LocalId> {
    let stem = path.file_stem().and_then(OsStr::to_str).ok_or_else(|| Error::Parse {
        path: path.display().to_string(),
        message: "filename is not valid UTF-8".into(),
    })?;

    let (before_slug, slug) = stem.rsplit_once('-').ok_or_else(|| Error::Parse {
        path: path.display().to_string(),
        message: "filename must be <prefix>-<T|#><n>-<slug>".into(),
    })?;

    if slug.is_empty() {
        return Err(Error::Parse {
            path: path.display().to_string(),
            message: "filename slug segment is empty".into(),
        });
    }

    let (entity_prefix, id_part) = before_slug.rsplit_once('-').ok_or_else(|| Error::Parse {
        path: path.display().to_string(),
        message: "filename must be <prefix>-<T|#><n>-<slug>".into(),
    })?;

    if entity_prefix.is_empty() {
        return Err(Error::Parse {
            path: path.display().to_string(),
            message: "filename prefix segment is empty".into(),
        });
    }

    let kind = if let Some(number) = id_part.strip_prefix('T') {
        let number = number.parse::<u64>().map_err(|_| Error::Parse {
            path: path.display().to_string(),
            message: format!("invalid temporary id `T{number}`"),
        })?;
        LocalIdKind::Temporary(number)
    } else if let Some(number) = id_part.strip_prefix('#') {
        let number = number.parse::<u64>().map_err(|_| Error::Parse {
            path: path.display().to_string(),
            message: format!("invalid github id `#{number}`"),
        })?;
        LocalIdKind::GitHub(number)
    } else {
        return Err(Error::Parse {
            path: path.display().to_string(),
            message: "filename id segment must be T<n> or #<n>".into(),
        });
    };

    Ok(LocalId { entity_prefix: entity_prefix.to_string(), slug: slug.to_string(), kind })
}

pub fn filename_stem(prefix: &str, slug: &str, kind: LocalIdKind) -> String {
    let id_part = match kind {
        LocalIdKind::Temporary(n) => format!("T{n}"),
        LocalIdKind::GitHub(n) => format!("#{n}"),
    };
    format!("{prefix}-{id_part}-{slug}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::IssueFrontmatter;

    #[test]
    fn parses_frontmatter() {
        let raw = r"---
id: app-implement_feature_a
repo: app
title: Test
state: open
labels: [roadmap]
milestone: milestone-build_ai_chatbot
project:
  status: Backlog
  priority: null
  start: null
  due: null
blocked_by: []
---
## Context
body
";
        let parsed = parse_markdown::<IssueFrontmatter>(raw, Path::new("app-T1-x.md")).unwrap();
        assert_eq!(parsed.frontmatter.id, "app-implement_feature_a");
        assert_eq!(parsed.body, "## Context\nbody");
    }

    #[test]
    fn parses_filename_ids() {
        let t = parse_filename_id(Path::new("app-T8-foo.md")).unwrap();
        assert_eq!(t.entity_prefix, "app");
        assert_eq!(t.slug, "foo");
        assert!(t.is_temporary());
        assert_eq!(t.github_number(), None);

        let mp = parse_filename_id(Path::new("my-platform-T5-auth_flow.md")).unwrap();
        assert_eq!(mp.entity_prefix, "my-platform");
        assert_eq!(mp.slug, "auth_flow");
        assert!(mp.is_temporary());

        let gh = parse_filename_id(Path::new("my-platform-#42-foo.md")).unwrap();
        assert_eq!(gh.entity_prefix, "my-platform");
        assert_eq!(gh.slug, "foo");
        assert!(!gh.is_temporary());
        assert_eq!(gh.github_number(), Some(42));

        let milestone = parse_filename_id(Path::new("milestone-#4-build_ai_chatbot.md")).unwrap();
        assert_eq!(milestone.entity_prefix, "milestone");
        assert_eq!(milestone.slug, "build_ai_chatbot");
        assert_eq!(milestone.github_number(), Some(4));
    }
}
