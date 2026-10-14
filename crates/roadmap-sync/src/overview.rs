//! Local markdown overview of milestones and issues (no network).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use crate::error::Result;
use crate::loader::load;
use crate::model::{Issue, IssueState, Milestone, Roadmap};

/// Filters for [`overview`].
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct OverviewFilter {
    /// Substring match against milestone id (OR). Empty = all.
    pub milestones: Vec<String>,
    /// Exact (case-insensitive) project `status` field values to include (OR).
    pub statuses: Vec<String>,
    /// Exact (case-insensitive) project `status` values to drop.
    pub exclude_statuses: Vec<String>,
    /// Substring match against milestone id to drop.
    pub exclude_milestones: Vec<String>,
    /// Issue `state` filter.
    pub state: Option<IssueState>,
    /// Issue `repo` filter (exact).
    pub repo: Option<String>,
    /// Only items with `start_date` or `target_date` set.
    pub dated: bool,
    /// Omit the milestones summary section.
    pub issues_only: bool,
    /// Omit issue rows.
    pub milestones_only: bool,
    /// Flat issue table instead of grouping under milestones.
    pub flat: bool,
}

/// Load roadmap and print a markdown overview to stdout.
pub fn overview(roadmap_dir: &Path, filter: &OverviewFilter) -> Result<()> {
    let roadmap = load(roadmap_dir)?;
    let markdown = render(&roadmap, filter);
    print!("{markdown}");
    Ok(())
}

/// Build markdown overview text (testable).
pub fn render(roadmap: &Roadmap, filter: &OverviewFilter) -> String {
    let milestones: Vec<&Milestone> =
        roadmap.milestones.iter().filter(|m| include_milestone(m, filter)).collect();
    let issues: Vec<&Issue> = roadmap.issues.iter().filter(|i| include_issue(i, filter)).collect();

    let mut out = String::new();
    writeln!(out, "# Roadmap overview").ok();
    writeln!(out).ok();
    if let Some(line) = filter_summary(filter) {
        writeln!(out, "_{line}_").ok();
        writeln!(out).ok();
    }
    writeln!(
        out,
        "{} milestone(s), {} issue(s)",
        milestones.len(),
        if filter.milestones_only { 0 } else { issues.len() }
    )
    .ok();
    writeln!(out).ok();

    if !filter.issues_only {
        write_milestones_section(&mut out, &milestones, roadmap);
        writeln!(out).ok();
    }

    if !filter.milestones_only {
        if filter.flat {
            write_issues_flat(&mut out, &issues);
        } else {
            write_issues_grouped(&mut out, &milestones, &issues, roadmap);
        }
    }

    out
}

fn include_milestone(m: &Milestone, filter: &OverviewFilter) -> bool {
    if !include_by_substring(m.id(), &filter.milestones) {
        return false;
    }
    if exclude_by_substring(m.id(), &filter.exclude_milestones) {
        return false;
    }
    let status = field_value(&m.file.frontmatter.fields, "status");
    if !status_allowed(status, &filter.statuses, &filter.exclude_statuses) {
        return false;
    }
    if filter.dated && !has_dates(&m.file.frontmatter.fields, m.file.frontmatter.target_date()) {
        return false;
    }
    true
}

fn include_issue(issue: &Issue, filter: &OverviewFilter) -> bool {
    if let Some(repo) = &filter.repo {
        if issue.file.frontmatter.repo != *repo {
            return false;
        }
    }
    if let Some(state) = filter.state {
        if issue.file.frontmatter.state != state {
            return false;
        }
    }

    let mid = issue.file.frontmatter.milestone.as_deref().unwrap_or("");
    if !include_by_substring(mid, &filter.milestones) {
        return false;
    }
    if exclude_by_substring(mid, &filter.exclude_milestones) {
        return false;
    }

    let status = Some(issue.project_status());
    if !status_allowed(status, &filter.statuses, &filter.exclude_statuses) {
        return false;
    }
    if filter.dated && !has_dates(&issue.file.frontmatter.fields, None) {
        return false;
    }
    true
}

fn status_allowed(status: Option<&str>, include: &[String], exclude: &[String]) -> bool {
    if !include.is_empty() {
        let Some(status) = status else {
            return false;
        };
        if !include.iter().any(|s| s.eq_ignore_ascii_case(status)) {
            return false;
        }
    }
    if let Some(status) = status {
        if exclude.iter().any(|s| s.eq_ignore_ascii_case(status)) {
            return false;
        }
    }
    true
}

/// Empty include list = keep all.
fn include_by_substring(value: &str, needles: &[String]) -> bool {
    needles.is_empty() || substring_hit(value, needles)
}

/// Empty exclude list = drop none.
fn exclude_by_substring(value: &str, needles: &[String]) -> bool {
    !needles.is_empty() && substring_hit(value, needles)
}

fn substring_hit(value: &str, needles: &[String]) -> bool {
    let haystack = value.to_ascii_lowercase();
    needles.iter().any(|n| haystack.contains(&n.to_ascii_lowercase()))
}

fn has_dates(fields: &BTreeMap<String, Option<String>>, top_target: Option<&str>) -> bool {
    field_value(fields, "start_date").is_some()
        || field_value(fields, "target_date").is_some()
        || top_target.is_some_and(|v| !v.is_empty())
}

fn field_value<'a>(fields: &'a BTreeMap<String, Option<String>>, key: &str) -> Option<&'a str> {
    fields.get(key).and_then(|v| v.as_deref()).filter(|v| !v.is_empty())
}

fn filter_summary(filter: &OverviewFilter) -> Option<String> {
    let mut parts = Vec::new();
    if !filter.milestones.is_empty() {
        parts.push(format!("milestone~{}", filter.milestones.join("|")));
    }
    if !filter.exclude_milestones.is_empty() {
        parts.push(format!("exclude-milestone~{}", filter.exclude_milestones.join("|")));
    }
    if !filter.statuses.is_empty() {
        parts.push(format!("status={}", filter.statuses.join("|")));
    }
    if !filter.exclude_statuses.is_empty() {
        parts.push(format!("exclude-status={}", filter.exclude_statuses.join("|")));
    }
    if let Some(state) = filter.state {
        parts.push(format!("state={}", state.as_str()));
    }
    if let Some(repo) = &filter.repo {
        parts.push(format!("repo={repo}"));
    }
    if filter.dated {
        parts.push("dated".into());
    }
    if filter.issues_only {
        parts.push("issues-only".into());
    }
    if filter.milestones_only {
        parts.push("milestones-only".into());
    }
    if filter.flat {
        parts.push("flat".into());
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!("Filters: {}", parts.join(", ")))
    }
}

fn write_milestones_section(out: &mut String, milestones: &[&Milestone], roadmap: &Roadmap) {
    writeln!(out, "## Milestones").ok();
    writeln!(out).ok();
    if milestones.is_empty() {
        writeln!(out, "_None_").ok();
        return;
    }

    writeln!(out, "| id | title | lifecycle | status | estimate | start | target | open/closed |")
        .ok();
    writeln!(out, "|----|-------|-----------|--------|----------|-------|--------|-------------|")
        .ok();

    let mut sorted = milestones.to_vec();
    sorted.sort_by_key(|m| epic_sort_key(&m.file.frontmatter.title));

    for m in sorted {
        let fields = &m.file.frontmatter.fields;
        let (open, closed) = count_issue_states(roadmap, m.id());
        writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} | {} | {}/{} |",
            m.id(),
            escape_cell(&m.file.frontmatter.title),
            m.file.frontmatter.status.as_str(),
            cell(field_value(fields, "status")),
            cell(field_value(fields, "estimate")),
            cell(field_value(fields, "start_date")),
            cell(m.file.frontmatter.target_date()),
            open,
            closed,
        )
        .ok();
    }
}

fn write_issues_grouped(
    out: &mut String,
    milestones: &[&Milestone],
    issues: &[&Issue],
    roadmap: &Roadmap,
) {
    writeln!(out, "## Issues by milestone").ok();
    writeln!(out).ok();

    let mut by_milestone: BTreeMap<String, Vec<&Issue>> = BTreeMap::new();
    let mut unassigned = Vec::new();
    for issue in issues {
        match issue.file.frontmatter.milestone.as_deref() {
            Some(mid) if !mid.is_empty() => {
                by_milestone.entry(mid.to_string()).or_default().push(*issue);
            }
            _ => unassigned.push(*issue),
        }
    }

    let mut ordered_ids: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();

    let mut milestone_order: Vec<&Milestone> = if milestones.is_empty() {
        roadmap.milestones.iter().collect()
    } else {
        milestones.to_vec()
    };
    milestone_order.sort_by_key(|m| epic_sort_key(&m.file.frontmatter.title));

    for m in &milestone_order {
        if by_milestone.contains_key(m.id()) && seen.insert(m.id().to_string()) {
            ordered_ids.push(m.id().to_string());
        }
    }
    for mid in by_milestone.keys() {
        if seen.insert(mid.clone()) {
            ordered_ids.push(mid.clone());
        }
    }

    if ordered_ids.is_empty() && unassigned.is_empty() {
        writeln!(out, "_None_").ok();
        return;
    }

    for mid in ordered_ids {
        let Some(group) = by_milestone.get_mut(&mid) else {
            continue;
        };
        group.sort_by_key(|i| issue_sort_key(i));

        let title = roadmap
            .milestones
            .iter()
            .find(|m| m.id() == mid.as_str())
            .map_or(mid.as_str(), |m| m.file.frontmatter.title.as_str());

        writeln!(out, "### {title} (`{mid}`)").ok();
        if let Some(m) = roadmap.milestones.iter().find(|m| m.id() == mid) {
            let fields = &m.file.frontmatter.fields;
            let meta = [
                field_value(fields, "status").map(|s| format!("status={s}")),
                field_value(fields, "estimate").map(|e| format!("estimate={e}")),
                schedule_range(field_value(fields, "start_date"), m.file.frontmatter.target_date()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            if !meta.is_empty() {
                writeln!(out, "{}", meta.join(" · ")).ok();
            }
        }
        writeln!(out).ok();
        write_issue_table(out, group);
        writeln!(out).ok();
    }

    if !unassigned.is_empty() {
        unassigned.sort_by_key(|i| issue_sort_key(i));
        writeln!(out, "### Unassigned").ok();
        writeln!(out).ok();
        write_issue_table(out, &unassigned);
        writeln!(out).ok();
    }
}

fn write_issues_flat(out: &mut String, issues: &[&Issue]) {
    writeln!(out, "## Issues").ok();
    writeln!(out).ok();
    if issues.is_empty() {
        writeln!(out, "_None_").ok();
        return;
    }

    let mut sorted = issues.to_vec();
    sorted.sort_by_key(|i| {
        (i.file.frontmatter.milestone.clone().unwrap_or_default(), issue_sort_key(i))
    });

    writeln!(out, "| id | state | milestone | status | estimate | start | target |").ok();
    writeln!(out, "|----|-------|-----------|--------|----------|-------|--------|").ok();
    for issue in sorted {
        let fields = &issue.file.frontmatter.fields;
        writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} | {} |",
            issue.id(),
            issue.file.frontmatter.state.as_str(),
            cell(issue.file.frontmatter.milestone.as_deref()),
            cell(Some(issue.project_status())),
            cell(field_value(fields, "estimate")),
            cell(field_value(fields, "start_date")),
            cell(field_value(fields, "target_date")),
        )
        .ok();
    }
}

fn write_issue_table(out: &mut String, issues: &[&Issue]) {
    writeln!(out, "| id | state | status | estimate | start | target | title |").ok();
    writeln!(out, "|----|-------|--------|----------|-------|--------|-------|").ok();
    for issue in issues {
        let fields = &issue.file.frontmatter.fields;
        writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} | {} |",
            issue.id(),
            issue.file.frontmatter.state.as_str(),
            cell(Some(issue.project_status())),
            cell(field_value(fields, "estimate")),
            cell(field_value(fields, "start_date")),
            cell(field_value(fields, "target_date")),
            escape_cell(&issue.file.frontmatter.title),
        )
        .ok();
    }
}

fn count_issue_states(roadmap: &Roadmap, milestone_id: &str) -> (usize, usize) {
    let mut open = 0usize;
    let mut closed = 0usize;
    for issue in &roadmap.issues {
        if issue.file.frontmatter.milestone.as_deref() != Some(milestone_id) {
            continue;
        }
        match issue.file.frontmatter.state {
            IssueState::Open => open += 1,
            IssueState::Closed => closed += 1,
        }
    }
    (open, closed)
}

fn epic_sort_key(title: &str) -> (u32, String) {
    if let Some(rest) = title.strip_prefix("Epic ") {
        if let Some((num, _)) = rest.split_once(':') {
            if let Ok(n) = num.trim().parse::<u32>() {
                return (n, title.to_ascii_lowercase());
            }
        }
    }
    (u32::MAX, title.to_ascii_lowercase())
}

fn issue_sort_key(issue: &Issue) -> (String, String) {
    let start = field_value(&issue.file.frontmatter.fields, "start_date")
        .unwrap_or("9999-99-99")
        .to_string();
    (start, issue.id().to_string())
}

fn schedule_range(start: Option<&str>, target: Option<&str>) -> Option<String> {
    match (start, target) {
        (Some(s), Some(t)) => Some(format!("{s} → {t}")),
        (Some(s), None) => Some(format!("start={s}")),
        (None, Some(t)) => Some(format!("target={t}")),
        (None, None) => None,
    }
}

fn cell(value: Option<&str>) -> String {
    value.unwrap_or("—").to_string()
}

fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;
    use crate::model::{
        IssueFile, IssueFrontmatter, MilestoneFile, MilestoneFrontmatter, MilestoneStatus, SyncMap,
    };
    use crate::parse::{LocalId, LocalIdKind};

    fn milestone(
        id: &str,
        title: &str,
        status: MilestoneStatus,
        fields: BTreeMap<String, Option<String>>,
    ) -> Milestone {
        Milestone {
            file: MilestoneFile {
                path: PathBuf::from(format!("milestones/{id}.md")),
                local_id: LocalId {
                    entity_prefix: "milestone".into(),
                    slug: id.trim_start_matches("milestone-").into(),
                    kind: LocalIdKind::Temporary(1),
                },
                frontmatter: MilestoneFrontmatter {
                    id: id.into(),
                    title: title.into(),
                    status,
                    target_date: field_value(&fields, "target_date").map(str::to_string),
                    issues: vec![],
                    fields,
                },
                body: String::new(),
            },
        }
    }

    fn issue(
        id: &str,
        repo: &str,
        title: &str,
        state: IssueState,
        milestone: Option<&str>,
        fields: BTreeMap<String, Option<String>>,
    ) -> Issue {
        Issue {
            file: IssueFile {
                path: PathBuf::from(format!("issues/{id}.md")),
                local_id: LocalId {
                    entity_prefix: repo.into(),
                    slug: id.split('-').next_back().unwrap_or(id).into(),
                    kind: LocalIdKind::Temporary(1),
                },
                frontmatter: IssueFrontmatter {
                    id: id.into(),
                    repo: repo.into(),
                    title: title.into(),
                    url: None,
                    state,
                    labels: vec![],
                    milestone: milestone.map(str::to_string),
                    blocked_by: vec![],
                    fields,
                },
                body: String::new(),
            },
        }
    }

    fn fields(pairs: &[(&str, &str)]) -> BTreeMap<String, Option<String>> {
        pairs.iter().map(|(k, v)| ((*k).into(), Some((*v).into()))).collect()
    }

    fn sample_roadmap() -> Roadmap {
        Roadmap {
            milestones: vec![
                milestone(
                    "milestone-extraction",
                    "Epic 1: Extraction",
                    MilestoneStatus::InProgress,
                    fields(&[
                        ("status", "Todo"),
                        ("estimate", "14.5"),
                        ("start_date", "2026-10-13"),
                        ("target_date", "2026-11-02"),
                    ]),
                ),
                milestone(
                    "milestone-ideas_icebox",
                    "Epic 8: Ideas icebox",
                    MilestoneStatus::Planned,
                    fields(&[("status", "Backlog"), ("estimate", "4")]),
                ),
                milestone(
                    "milestone-facts",
                    "Epic 2: Facts",
                    MilestoneStatus::Planned,
                    fields(&[
                        ("status", "Todo"),
                        ("estimate", "8.5"),
                        ("start_date", "2026-11-03"),
                        ("target_date", "2026-11-13"),
                    ]),
                ),
            ],
            issues: vec![
                issue(
                    "platform-introduce_extraction",
                    "platform",
                    "Introduce extraction models",
                    IssueState::Open,
                    Some("milestone-extraction"),
                    fields(&[
                        ("status", "Todo"),
                        ("estimate", "3"),
                        ("start_date", "2026-10-16"),
                        ("target_date", "2026-10-20"),
                    ]),
                ),
                issue(
                    "platform-formalize_schema",
                    "platform",
                    "Formalize schema",
                    IssueState::Closed,
                    Some("milestone-extraction"),
                    fields(&[
                        ("status", "Done"),
                        ("estimate", "2.5"),
                        ("start_date", "2026-10-13"),
                        ("target_date", "2026-10-15"),
                    ]),
                ),
                issue(
                    "platform-icebox_idea",
                    "platform",
                    "Some idea",
                    IssueState::Open,
                    Some("milestone-ideas_icebox"),
                    fields(&[("status", "Backlog"), ("estimate", "1")]),
                ),
                issue(
                    "platform-facts_page",
                    "platform",
                    "Facts explore page",
                    IssueState::Open,
                    Some("milestone-facts"),
                    fields(&[
                        ("status", "Todo"),
                        ("estimate", "3.5"),
                        ("start_date", "2026-11-06"),
                        ("target_date", "2026-11-11"),
                    ]),
                ),
            ],
            sync_map: SyncMap::default(),
        }
    }

    #[test]
    fn render_default_groups_by_epic_order() {
        let md = render(&sample_roadmap(), &OverviewFilter::default());
        assert!(md.contains("# Roadmap overview"));
        assert!(md.contains("## Milestones"));
        assert!(md.contains("## Issues by milestone"));
        // Epic 1 before Epic 2 before Epic 8
        let e1 = md.find("Epic 1: Extraction").expect("epic 1");
        let e2 = md.find("Epic 2: Facts").expect("epic 2");
        let e8 = md.find("Epic 8: Ideas icebox").expect("epic 8");
        assert!(e1 < e2 && e2 < e8);
        assert!(md.contains("`platform-introduce_extraction`"));
    }

    #[test]
    fn filter_status_and_exclude_icebox() {
        let filter = OverviewFilter {
            statuses: vec!["Todo".into()],
            exclude_milestones: vec!["ideas".into()],
            ..OverviewFilter::default()
        };
        let md = render(&sample_roadmap(), &filter);
        assert!(md.contains("Filters:"));
        assert!(md.contains("`milestone-extraction`"));
        assert!(md.contains("`milestone-facts`"));
        assert!(!md.contains("ideas_icebox"));
        assert!(!md.contains("platform-icebox_idea"));
        assert!(md.contains("`platform-introduce_extraction`"));
        // Done issue excluded by status=Todo
        assert!(!md.contains("platform-formalize_schema"));
    }

    #[test]
    fn filter_dated_open_flat() {
        let filter = OverviewFilter {
            dated: true,
            state: Some(IssueState::Open),
            flat: true,
            issues_only: true,
            ..OverviewFilter::default()
        };
        let md = render(&sample_roadmap(), &filter);
        assert!(!md.contains("## Milestones"));
        assert!(md.contains("## Issues"));
        assert!(md.contains("`platform-introduce_extraction`"));
        assert!(md.contains("`platform-facts_page`"));
        assert!(!md.contains("platform-formalize_schema")); // closed
        assert!(!md.contains("platform-icebox_idea")); // no dates
    }

    #[test]
    fn filter_milestone_substring() {
        let filter = OverviewFilter {
            milestones: vec!["extraction".into()],
            milestones_only: true,
            ..OverviewFilter::default()
        };
        let md = render(&sample_roadmap(), &filter);
        assert!(md.contains("`milestone-extraction`"));
        assert!(!md.contains("`milestone-facts`"));
        assert!(!md.contains("## Issues"));
    }
}
