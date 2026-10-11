use std::collections::BTreeSet;
use std::path::Path;

use crate::config::RoadmapConfig;
use crate::error::Result;
use crate::field_sync::{
    field_values_equal, normalize_field_value, resolve_field_target, FieldTarget,
};
use crate::loader::load;
use crate::model::{Issue, Roadmap};
use crate::platform::{IssueRef, Platform, ProjectRef};
use crate::prefetch::ProjectItemState;
use crate::style;

/// Inspect one GitHub issue: local markdown vs remote issue fields + project fields.
pub async fn show_issue(
    roadmap_dir: &Path,
    platform: &dyn Platform,
    issue_number: u64,
    repo_filter: Option<&str>,
) -> Result<()> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let roadmap = load(roadmap_dir)?;
    let github = config.github()?;
    let project = ProjectRef { owner: github.owner.clone(), number: github.project_number };

    let matches = find_local_issues(&roadmap, &config, issue_number, repo_filter)?;
    let local = select_local_issue(matches, issue_number, repo_filter)?;

    let full_repo = match &local {
        Some(issue) => config.full_repo(&issue.file.frontmatter.repo)?,
        None => resolve_remote_repo(&config, repo_filter)?,
    };

    let issue_ref = IssueRef { repo: full_repo.clone(), number: issue_number };
    let issue_url = format!("https://github.com/{full_repo}/issues/{issue_number}");

    println!("{}", style::heading("show"));
    println!("  {} {full_repo}#{issue_number}", style::tag("issue"));
    println!("  {} {issue_url}", style::tag("url"));

    let remote = platform.get_issue(&issue_ref).await?;
    let issue_field_values = platform.get_issue_field_values(&issue_ref).await?;
    let issue_field_defs = platform.list_issue_fields(&full_repo).await?;
    let project_meta = platform.load_project_meta(&project).await?;
    let project_items = platform.list_project_items(&project).await?;
    let project_item = project_items.iter().find(|item| item.issue_url == issue_url).map(|item| {
        ProjectItemState { item_id: item.item_id.clone(), field_values: item.field_values.clone() }
    });

    println!();
    println!("{}", style::heading("remote issue"));
    println!("  title: {}", remote.title);
    println!("  state: {}", remote.state);
    if let Some(parent) = &remote.parent_url {
        println!("  parent: {parent}");
    } else {
        println!("  parent: —");
    }

    println!();
    println!("{}", style::heading("issue fields (GitHub Issue Fields)"));
    if issue_field_values.is_empty() {
        println!("  (none)");
    } else {
        let mut names: Vec<_> = issue_field_values.keys().collect();
        names.sort();
        for name in names {
            println!(
                "  {name}: {}",
                display_value(issue_field_values.get(name).map(String::as_str))
            );
        }
    }

    println!();
    println!("{}", style::heading(format!("project fields (project #{})", project.number)));
    match &project_item {
        None => println!("  (issue not on project)"),
        Some(item) => {
            println!("  item_id: {}", item.item_id);
            if item.field_values.is_empty() {
                println!("  (no field values set)");
            } else {
                let mut rows: Vec<_> = project_meta
                    .fields
                    .iter()
                    .filter_map(|field| {
                        item.field_values.get(&field.id).map(|value| {
                            (
                                field.name.as_str(),
                                field.id.as_str(),
                                field.field_type.as_str(),
                                value.as_str(),
                            )
                        })
                    })
                    .collect();
                // Orphan values whose field id is missing from meta.
                for (field_id, value) in &item.field_values {
                    if !project_meta.fields.iter().any(|field| field.id == *field_id) {
                        rows.push(("?", field_id.as_str(), "?", value.as_str()));
                    }
                }
                rows.sort_by(|a, b| a.0.cmp(b.0));
                for (name, id, field_type, value) in rows {
                    println!("  {name} [{field_type}] ({id}): {value}");
                }
            }
        }
    }

    println!();
    println!("{}", style::heading("project field catalog"));
    let mut catalog: Vec<_> = project_meta.fields.iter().collect();
    catalog.sort_by(|a, b| a.name.cmp(&b.name));
    for field in catalog {
        let type_label =
            if field.field_type.is_empty() { "∅" } else { field.field_type.as_str() };
        println!("  {} [{type_label}] {}", field.name, field.id);
    }

    println!();
    println!("{}", style::heading("local"));
    match &local {
        None => println!("  (no local markdown for this issue)"),
        Some(issue) => {
            println!("  id: {}", issue.id());
            println!("  path: {}", issue.file.path.display());
            println!("  title: {}", issue.file.frontmatter.title);
            println!("  state: {:?}", issue.file.frontmatter.state);
            if issue.file.frontmatter.fields.is_empty() {
                println!("  fields: (none)");
            } else {
                println!("  fields:");
                for (key, value) in &issue.file.frontmatter.fields {
                    println!("    {key}: {}", display_value(value.as_deref()));
                }
            }
        }
    }

    println!();
    println!("{}", style::heading("sync"));
    let mappings = &roadmap.sync_map.meta.fields.issue;
    let local_keys: BTreeSet<String> = local
        .as_ref()
        .map(|issue| issue.file.frontmatter.fields.keys().cloned().collect())
        .unwrap_or_default();
    let mapping_keys: BTreeSet<String> = mappings.keys().cloned().collect();
    let mut keys: BTreeSet<String> = local_keys.union(&mapping_keys).cloned().collect();
    keys.insert("status".into());

    let mut rows = Vec::new();
    for key in &keys {
        let local_value = local
            .as_ref()
            .and_then(|issue| issue.file.frontmatter.fields.get(key))
            .and_then(|value| value.as_deref());
        let local_norm = normalize_field_value(local_value);
        let target = resolve_field_target(
            key,
            mappings,
            Some(issue_field_defs.as_slice()),
            Some(&project_meta),
        );

        let row = match target {
            None if key == "status" => {
                let status_field = project_meta.field_by_name("Status");
                let remote = status_field.and_then(|field| {
                    project_item.as_ref().and_then(|item| item.field_values.get(&field.id))
                });
                let remote_norm = normalize_field_value(remote.map(String::as_str));
                let desired = local.as_ref().map_or("Backlog", |issue| issue.project_status());
                let desired_norm = normalize_field_value(Some(desired));
                let in_sync = desired_norm == remote_norm;
                SyncRow {
                    kind: if in_sync { SyncKind::Ok } else { SyncKind::Unsync },
                    key: key.clone(),
                    route: "project / \"Status\"".into(),
                    detail: value_detail(desired_norm, remote_norm, in_sync),
                }
            }
            None => {
                let mapping = mappings.get(key.as_str());
                let detail = match mapping {
                    None => "no sync-map mapping / issue-field definition".to_string(),
                    Some(mapping) => {
                        let map_type =
                            if mapping.field_type.is_empty() { "∅" } else { &mapping.field_type };
                        let source = match mapping.effective_source() {
                            Some(source) => format!("{source:?}").to_ascii_lowercase(),
                            None => "none".into(),
                        };
                        format!(
                            "source={source}, map_type={map_type}, field_id={}",
                            mapping.field_id
                        )
                    }
                };
                let meta_type = mapping.and_then(|mapping| {
                    project_meta
                        .fields
                        .iter()
                        .find(|field| field.id == mapping.field_id)
                        .map(|field| field.field_type.as_str())
                });
                let route = match (mapping, meta_type) {
                    (Some(mapping), Some(meta_type)) if !meta_type.is_empty() => {
                        format!("project / \"{}\" [{meta_type}]", mapping.field_name)
                    }
                    (Some(mapping), _) => format!("project / \"{}\"", mapping.field_name),
                    _ => "—".into(),
                };
                SyncRow { kind: SyncKind::Skip, key: key.clone(), route, detail }
            }
            Some(target) => {
                let (route, remote_norm) = match &target {
                    FieldTarget::Issue { field_name } => (
                        format!("issue / \"{field_name}\""),
                        issue_field_values
                            .get(field_name)
                            .and_then(|value| normalize_field_value(Some(value.as_str()))),
                    ),
                    FieldTarget::Project { field_id, field_name, field_type } => (
                        format!("project / \"{field_name}\" [{field_type}]"),
                        project_item
                            .as_ref()
                            .and_then(|item| item.field_values.get(field_id))
                            .and_then(|value| normalize_field_value(Some(value.as_str()))),
                    ),
                };
                let in_sync = field_values_equal(&target, local_norm, remote_norm);
                SyncRow {
                    kind: if in_sync { SyncKind::Ok } else { SyncKind::Unsync },
                    key: key.clone(),
                    route,
                    detail: value_detail(local_norm, remote_norm, in_sync),
                }
            }
        };
        rows.push(row);
    }

    rows.sort_by(|left, right| {
        left.kind.sort_key().cmp(&right.kind.sort_key()).then_with(|| left.key.cmp(&right.key))
    });

    let mut unsynced = 0usize;
    let mut skipped = 0usize;
    for row in &rows {
        match row.kind {
            SyncKind::Unsync => unsynced += 1,
            SyncKind::Skip => skipped += 1,
            SyncKind::Ok => {}
        }
        print_sync_row(row);
    }

    println!();
    if unsynced == 0 && skipped == 0 {
        println!("{}", style::ok("all mapped fields in sync"));
    } else {
        println!(
            "{} — {unsynced} unsynced, {skipped} skipped",
            if unsynced > 0 { style::warn("drift") } else { style::ok("ok") }
        );
    }

    println!();
    println!("{}", style::heading("remote body"));
    print_indented_body(&remote.body);

    if let Some(issue) = &local {
        if issue.file.body != remote.body {
            println!();
            println!("{}", style::heading("local body (differs)"));
            print_indented_body(&issue.file.body);
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncKind {
    Unsync,
    Ok,
    Skip,
}

impl SyncKind {
    fn sort_key(self) -> u8 {
        match self {
            Self::Unsync => 0,
            Self::Ok => 1,
            Self::Skip => 2,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Unsync => "unsync",
            Self::Ok => "ok",
            Self::Skip => "skip",
        }
    }
}

struct SyncRow {
    kind: SyncKind,
    key: String,
    route: String,
    detail: String,
}

fn print_sync_row(row: &SyncRow) {
    let tag = match row.kind {
        SyncKind::Ok => style::ok(format!("[{}]", row.kind.label())),
        SyncKind::Unsync => style::err(format!("[{}]", row.kind.label())),
        SyncKind::Skip => style::warn(format!("[{}]", row.kind.label())),
    };
    // Pad to width of "[unsync]" for alignment (ANSI codes excluded from pad).
    let pad = " ".repeat(8usize.saturating_sub(row.kind.label().len() + 2));
    let key = format!("{:<14}", row.key);
    let line = format!("  - {tag}{pad} {key} -> {}", row.route);
    if row.detail.is_empty() {
        println!("{line}");
    } else {
        println!("{line} / {}", style::dim(&row.detail));
    }
}

fn value_detail(local: Option<&str>, remote: Option<&str>, in_sync: bool) -> String {
    if in_sync {
        String::new()
    } else {
        format!("local={} remote={}", display_value(local), display_value(remote))
    }
}

fn display_value(value: Option<&str>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "—".into(),
    }
}

fn print_indented_body(body: &str) {
    if body.trim().is_empty() {
        println!("  (empty)");
        return;
    }
    for line in body.lines() {
        println!("  {line}");
    }
}

fn find_local_issues<'a>(
    roadmap: &'a Roadmap,
    config: &RoadmapConfig,
    number: u64,
    repo_filter: Option<&str>,
) -> Result<Vec<&'a Issue>> {
    let mut matches = Vec::new();
    for issue in &roadmap.issues {
        if issue.github_number() != Some(number) {
            continue;
        }
        if let Some(repo_id) = repo_filter {
            if issue.file.frontmatter.repo != repo_id {
                continue;
            }
        }
        // Ensure repo id is valid.
        let _ = config.full_repo(&issue.file.frontmatter.repo)?;
        matches.push(issue);
    }
    Ok(matches)
}

fn select_local_issue<'a>(
    matches: Vec<&'a Issue>,
    number: u64,
    _repo_filter: Option<&str>,
) -> Result<Option<&'a Issue>> {
    match matches.as_slice() {
        [] => Ok(None),
        [issue] => Ok(Some(*issue)),
        many => {
            let repos: Vec<_> =
                many.iter().map(|issue| issue.file.frontmatter.repo.as_str()).collect();
            Err(crate::error::Error::Config(format!(
                "issue #{number} matches multiple local files (repos: {}). Pass --repo <id>.",
                repos.join(", ")
            )))
        }
    }
}

fn resolve_remote_repo(config: &RoadmapConfig, repo_filter: Option<&str>) -> Result<String> {
    if let Some(repo_id) = repo_filter {
        return config.full_repo(repo_id);
    }
    let github = config.github()?;
    if github.repos.len() == 1 {
        let repo_id = github.repos.keys().next().expect("len == 1");
        return config.full_repo(repo_id);
    }
    Err(crate::error::Error::Config(
        "no local issue matched; pass --repo <id> to select the GitHub repository".into(),
    ))
}
