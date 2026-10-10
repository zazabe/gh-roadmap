use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::stream::{self, StreamExt};
use futures::TryStreamExt;

use crate::config::RoadmapConfig;
use crate::error::Result;
use crate::model::Roadmap;
use crate::platform::{
    IssueFieldDefinition, IssueRef, Platform, ProjectMeta, ProjectRef, RemoteIssue,
    RemoteIssueSnapshot,
};
use crate::style;

#[derive(Debug, Clone, Default)]
pub struct RemoteCache {
    pub issues: HashMap<IssueRef, RemoteIssue>,
    pub issue_fields: HashMap<IssueRef, HashMap<String, String>>,
    pub issue_field_defs_by_repo: HashMap<String, Vec<IssueFieldDefinition>>,
    pub project_items: HashMap<String, ProjectItemState>,
    pub project_meta: Option<ProjectMeta>,
}

#[derive(Debug, Clone)]
pub struct ProjectItemState {
    pub item_id: String,
    pub field_values: HashMap<String, String>,
}

pub async fn fetch(
    roadmap: &Roadmap,
    config: &RoadmapConfig,
    platform: &dyn Platform,
) -> Result<RemoteCache> {
    let github = config.github()?;
    let project = ProjectRef { owner: github.owner.clone(), number: github.project_number };

    let issue_refs = collect_issue_refs(roadmap, config)?;

    println!("{}", style::heading("prefetch"));
    flush();

    print_phase(&format!("fetching {total} linked issue snapshot(s)", total = issue_refs.len()));
    let (issues, issue_fields) = fetch_issue_snapshots(platform, &issue_refs).await?;
    print_phase_ok(&format!("{} issue(s)", issues.len()));

    print_phase("loading issue field definitions");
    let issue_field_defs_by_repo = fetch_issue_field_defs(platform, &issue_refs).await?;
    print_phase_ok(&format!("{} repo(s)", issue_field_defs_by_repo.len()));

    print_phase(&format!("listing project #{} items", project.number));
    let project_items_raw = platform.list_project_items(&project).await?;
    let project_count = project_items_raw.len();
    let project_items: HashMap<String, ProjectItemState> = project_items_raw
        .into_iter()
        .map(|item| {
            (
                item.issue_url,
                ProjectItemState { item_id: item.item_id, field_values: item.field_values },
            )
        })
        .collect();
    print_phase_ok(&format!("{project_count} item(s)"));

    print_phase("loading project field metadata");
    let project_meta = Some(platform.load_project_meta(&project).await?);
    print_phase_ok("ready");

    println!(
        "{} — {} issue(s), {} issue field snapshot(s), {} project item(s)",
        style::ok("prefetch complete"),
        issues.len(),
        issue_fields.len(),
        project_items.len()
    );
    flush();

    Ok(RemoteCache { issues, issue_fields, issue_field_defs_by_repo, project_items, project_meta })
}

async fn fetch_issue_field_defs(
    platform: &dyn Platform,
    issue_refs: &[IssueRef],
) -> Result<HashMap<String, Vec<IssueFieldDefinition>>> {
    let mut repos: Vec<String> =
        issue_refs.iter().map(|issue_ref| issue_ref.repo.clone()).collect();
    repos.sort();
    repos.dedup();
    let pairs: Vec<_> = stream::iter(repos)
        .map(|repo| async move {
            let fields = platform.list_issue_fields(&repo).await?;
            Ok::<_, crate::error::Error>((repo, fields))
        })
        .buffer_unordered(8)
        .try_collect()
        .await?;
    Ok(pairs.into_iter().collect())
}

async fn fetch_issue_snapshots(
    platform: &dyn Platform,
    issue_refs: &[IssueRef],
) -> Result<(HashMap<IssueRef, RemoteIssue>, HashMap<IssueRef, HashMap<String, String>>)> {
    if issue_refs.is_empty() {
        return Ok((HashMap::new(), HashMap::new()));
    }
    let mut by_repo: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for issue_ref in issue_refs {
        by_repo.entry(issue_ref.repo.clone()).or_default().push(issue_ref.number);
    }
    for numbers in by_repo.values_mut() {
        numbers.sort_unstable();
        numbers.dedup();
    }
    let done = Arc::new(AtomicUsize::new(0));
    let total = by_repo.len();
    let repo_pairs: Vec<_> = stream::iter(by_repo)
        .map(|(repo, numbers)| {
            let done = Arc::clone(&done);
            async move {
                let snapshots = platform.get_issues_snapshot(&repo, &numbers).await?;
                let current = done.fetch_add(1, Ordering::Relaxed) + 1;
                println!(
                    "  [{current}/{total}] {repo} ({count} issue(s))",
                    count = snapshots.len()
                );
                flush();
                Ok::<_, crate::error::Error>((repo, snapshots))
            }
        })
        .buffer_unordered(8)
        .try_collect()
        .await?;
    let mut issues = HashMap::new();
    let mut issue_fields = HashMap::new();
    for (repo, snapshots) in repo_pairs {
        for (number, snapshot) in snapshots {
            let issue_ref = IssueRef { repo: repo.clone(), number };
            let RemoteIssueSnapshot { issue, field_values } = snapshot;
            issues.insert(issue_ref.clone(), issue);
            issue_fields.insert(issue_ref, field_values);
        }
    }
    Ok((issues, issue_fields))
}

fn print_phase(detail: &str) {
    println!("  {} {}", style::emphasis("…"), detail);
    flush();
}

fn print_phase_ok(result: &str) {
    println!("      {} — {result}", style::status_ok());
    flush();
}

fn flush() {
    let _ = std::io::stdout().flush();
}

fn collect_issue_refs(roadmap: &Roadmap, config: &RoadmapConfig) -> Result<Vec<IssueRef>> {
    let hub_full_repo = config.hub_full_repo()?;
    let mut refs = Vec::new();

    for milestone in &roadmap.milestones {
        if let Some(number) = milestone.github_number() {
            refs.push(IssueRef { repo: hub_full_repo.clone(), number });
        }
    }

    for issue in &roadmap.issues {
        if let Some(number) = issue.github_number() {
            let full_repo = config.full_repo(&issue.file.frontmatter.repo)?;
            refs.push(IssueRef { repo: full_repo, number });
        }
    }

    refs.sort_by(|left, right| left.repo.cmp(&right.repo).then(left.number.cmp(&right.number)));
    refs.dedup();
    Ok(refs)
}

pub fn issue_needs_update(
    desired_title: &str,
    desired_body: &str,
    desired_state: &str,
    remote: &RemoteIssue,
) -> bool {
    desired_title != remote.title
        || desired_body != remote.body
        || !desired_state.eq_ignore_ascii_case(&remote.state)
}

pub fn parent_already_linked(remote: &RemoteIssue, expected_parent_url: &str) -> bool {
    remote.parent_url.as_deref() == Some(expected_parent_url)
}

pub fn project_item_for_url<'a>(
    cache: &'a RemoteCache,
    issue_url: &str,
) -> Option<&'a ProjectItemState> {
    cache.project_items.get(issue_url)
}

pub fn project_status_matches(
    item: &ProjectItemState,
    status_field_id: &str,
    desired_status: &str,
) -> bool {
    item.field_values.get(status_field_id).is_some_and(|status| status == desired_status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_issue(title: &str, body: &str, state: &str) -> RemoteIssue {
        RemoteIssue {
            number: 1,
            url: "https://github.com/o/r/issues/1".into(),
            title: title.into(),
            body: body.into(),
            state: state.into(),
            parent_url: None,
        }
    }

    #[test]
    fn issue_needs_update_detects_title_change() {
        let remote = remote_issue("old", "body", "open");
        assert!(issue_needs_update("new", "body", "open", &remote));
    }

    #[test]
    fn issue_needs_update_ignores_case_on_state() {
        let remote = remote_issue("t", "b", "OPEN");
        assert!(!issue_needs_update("t", "b", "open", &remote));
    }

    #[test]
    fn parent_already_linked_matches_url() {
        let mut remote = remote_issue("t", "b", "open");
        remote.parent_url = Some("https://github.com/o/r/issues/3".into());
        assert!(parent_already_linked(&remote, "https://github.com/o/r/issues/3"));
        assert!(!parent_already_linked(&remote, "https://github.com/o/r/issues/4"));
    }
}
