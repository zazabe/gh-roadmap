use std::path::{Path, PathBuf};

use crate::config::{default_sync_map_path, RoadmapConfig};
use crate::error::{Error, Result};
use crate::model::{Issue, Milestone, Roadmap, SyncMap};
use crate::parse::parse_markdown_file;

pub fn load(roadmap_dir: &Path) -> Result<Roadmap> {
    let _config = RoadmapConfig::load(roadmap_dir)?;
    let milestones = load_milestones(&roadmap_dir.join("milestones"))?;
    let issues = load_issues(&roadmap_dir.join("issues"), roadmap_dir)?;
    let sync_map = SyncMap::load(&default_sync_map_path(roadmap_dir))?;

    Ok(Roadmap { milestones, issues, sync_map })
}

fn load_milestones(dir: &Path) -> Result<Vec<Milestone>> {
    let mut milestones = Vec::new();
    if !dir.exists() {
        return Ok(milestones);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !is_markdown_file(&path) {
            continue;
        }
        let parsed = parse_markdown_file::<crate::model::MilestoneFrontmatter>(&path)?;
        if parsed.local_id.entity_prefix != "milestone" {
            return Err(parse_error(&path, "milestone filename must start with `milestone-`"));
        }
        milestones.push(Milestone {
            file: crate::model::MilestoneFile {
                path,
                local_id: parsed.local_id,
                frontmatter: parsed.frontmatter,
                body: parsed.body,
            },
        });
    }

    milestones.sort_by(|a, b| a.id().cmp(b.id()));
    Ok(milestones)
}

fn load_issues(dir: &Path, roadmap_dir: &Path) -> Result<Vec<Issue>> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let mut issues = Vec::new();
    if !dir.exists() {
        return Ok(issues);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !is_markdown_file(&path) {
            continue;
        }
        let parsed = parse_markdown_file::<crate::model::IssueFrontmatter>(&path)?;
        if !config.github()?.repos.contains_key(&parsed.frontmatter.repo) {
            return Err(parse_error(&path, format!("unknown repo `{}`", parsed.frontmatter.repo)));
        }
        if parsed.local_id.entity_prefix != parsed.frontmatter.repo {
            return Err(parse_error(
                &path,
                format!(
                    "issue filename prefix `{}` does not match repo `{}`",
                    parsed.local_id.entity_prefix, parsed.frontmatter.repo
                ),
            ));
        }
        issues.push(Issue {
            file: crate::model::IssueFile {
                path,
                local_id: parsed.local_id,
                frontmatter: parsed.frontmatter,
                body: parsed.body,
            },
        });
    }

    issues.sort_by(|a, b| a.id().cmp(b.id()));
    Ok(issues)
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "md")
}

fn parse_error(path: &Path, message: impl Into<String>) -> Error {
    Error::Parse { path: path.display().to_string(), message: message.into() }
}

/// Rename `<repo>-T<n>-<slug>.md` to `<repo>-#<github_number>-<slug>.md` after create.
pub fn linked_issue_path(path: &Path, github_number: u64) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parsed = crate::parse::parse_filename_id(path)
        .unwrap_or_else(|_| panic!("linked_issue_path: invalid filename {}", path.display()));
    parent.join(format!(
        "{}.md",
        crate::parse::filename_stem(
            &parsed.entity_prefix,
            &parsed.slug,
            crate::parse::LocalIdKind::GitHub(github_number),
        )
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_issue_path_works() {
        let path = Path::new("roadmap/issues/app-T8-implement_feature_a.md");
        assert_eq!(
            linked_issue_path(path, 42),
            PathBuf::from("roadmap/issues/app-#42-implement_feature_a.md")
        );

        let path = Path::new("roadmap/issues/my-platform-T5-auth_flow.md");
        assert_eq!(
            linked_issue_path(path, 91),
            PathBuf::from("roadmap/issues/my-platform-#91-auth_flow.md")
        );
    }
}
