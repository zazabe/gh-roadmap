use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use futures::stream::{self, StreamExt};
use futures::TryStreamExt;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::config::{default_sync_map_path, RoadmapConfig};
use crate::error::Result;
use crate::loader::load;
use crate::model::{
    Issue, IssueFrontmatter, IssueState, IssueSyncEntry, Milestone, MilestoneFrontmatter,
    MilestoneStatus, ProjectFieldMapping, Roadmap, SyncMap,
};
use crate::platform::{IssueRef, Platform, ProjectMeta, ProjectRef, RemoteIssueSnapshot};
use crate::prefetch::ProjectItemState;
use crate::style;
use crate::validate::validate_or_err;
use crate::write::write_markdown_file;

#[derive(Debug, Clone)]
pub struct PullChange {
    pub id: String,
    pub path: String,
    pub field: String,
    pub local: Option<String>,
    pub remote: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PullPlan {
    pub changes: Vec<PullChange>,
    pub missing_issue_urls: Vec<String>,
    pub missing_milestone_urls: Vec<String>,
}

impl PullPlan {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
            && self.missing_issue_urls.is_empty()
            && self.missing_milestone_urls.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
struct ImportReport {
    created_issues: usize,
    touched_milestones: usize,
}

#[derive(Debug, Clone)]
struct PendingMilestone {
    frontmatter: MilestoneFrontmatter,
    path: PathBuf,
    body: String,
}

#[derive(Debug, Clone)]
struct ParsedIssueUrl {
    full_repo: String,
    number: u64,
}

pub fn build_pull_plan(
    roadmap: &Roadmap,
    config: &RoadmapConfig,
    sync_map: &SyncMap,
    project_items: &HashMap<String, ProjectItemState>,
    issue_fields_by_url: &HashMap<String, HashMap<String, String>>,
    snapshots_by_url: &HashMap<String, RemoteIssueSnapshot>,
    overwrite: bool,
) -> PullPlan {
    let hub_full_repo = config.hub_full_repo().ok();
    let mut changes = Vec::new();

    for issue in &roadmap.issues {
        if let Some(change_set) = issue_pull_changes(
            issue,
            config,
            sync_map,
            project_items,
            issue_fields_by_url,
            snapshots_by_url,
            overwrite,
        ) {
            changes.extend(change_set);
        }
    }

    if let Some(hub_full_repo) = hub_full_repo {
        for milestone in &roadmap.milestones {
            if let Some(change_set) = milestone_pull_changes(
                milestone,
                &hub_full_repo,
                sync_map,
                project_items,
                issue_fields_by_url,
                snapshots_by_url,
                overwrite,
            ) {
                changes.extend(change_set);
            }
        }
    }

    let (missing_issue_urls, missing_milestone_urls) =
        find_missing_urls(roadmap, config, project_items);
    PullPlan { changes, missing_issue_urls, missing_milestone_urls }
}

fn issue_pull_changes(
    issue: &Issue,
    config: &RoadmapConfig,
    sync_map: &SyncMap,
    project_items: &HashMap<String, ProjectItemState>,
    issue_fields_by_url: &HashMap<String, HashMap<String, String>>,
    snapshots_by_url: &HashMap<String, RemoteIssueSnapshot>,
    overwrite: bool,
) -> Option<Vec<PullChange>> {
    let number = issue.github_number()?;
    let full_repo = config.full_repo(&issue.file.frontmatter.repo).ok()?;
    let issue_url = format!("https://github.com/{full_repo}/issues/{number}");
    let mut changes = Vec::new();
    let path = issue.file.path.display().to_string();
    for (local_key, mapping) in &sync_map.meta.fields.issue {
        let local = issue.file.frontmatter.fields.get(local_key).cloned().flatten();
        let remote_value =
            resolve_remote_value(mapping, &issue_url, project_items, issue_fields_by_url);
        push_change(&mut changes, issue.id(), &path, local_key, local, remote_value, overwrite);
    }
    let local_state = Some(issue_state_to_str(issue.file.frontmatter.state).to_string());
    let remote_state = snapshots_by_url
        .get(&issue_url)
        .and_then(|snapshot| parse_issue_state(Some(snapshot.issue.state.as_str())))
        .map(|state| issue_state_to_str(state).to_string());
    push_state_change(&mut changes, issue.id(), &path, local_state, remote_state);
    let local_body = normalize_value(Some(issue.file.body.as_str())).map(str::to_string);
    let remote_body = snapshots_by_url.get(&issue_url).and_then(|snapshot| {
        normalize_value(Some(snapshot.issue.body.as_str())).map(str::to_string)
    });
    push_body_change(&mut changes, issue.id(), &path, local_body, remote_body, overwrite);
    if changes.is_empty() {
        None
    } else {
        Some(changes)
    }
}

fn milestone_pull_changes(
    milestone: &Milestone,
    hub_full_repo: &str,
    sync_map: &SyncMap,
    project_items: &HashMap<String, ProjectItemState>,
    issue_fields_by_url: &HashMap<String, HashMap<String, String>>,
    snapshots_by_url: &HashMap<String, RemoteIssueSnapshot>,
    overwrite: bool,
) -> Option<Vec<PullChange>> {
    let number = milestone.github_number()?;
    let issue_url = format!("https://github.com/{hub_full_repo}/issues/{number}");
    let mut changes = Vec::new();
    let path = milestone.file.path.display().to_string();
    for (local_key, mapping) in &sync_map.meta.fields.milestone {
        let local = milestone.file.frontmatter.fields.get(local_key).cloned().flatten();
        let remote_value =
            resolve_remote_value(mapping, &issue_url, project_items, issue_fields_by_url);
        push_change(&mut changes, milestone.id(), &path, local_key, local, remote_value, overwrite);
    }
    let local_body = normalize_value(Some(milestone.file.body.as_str())).map(str::to_string);
    let remote_body = snapshots_by_url.get(&issue_url).and_then(|snapshot| {
        normalize_value(Some(snapshot.issue.body.as_str())).map(str::to_string)
    });
    push_body_change(&mut changes, milestone.id(), &path, local_body, remote_body, overwrite);
    if changes.is_empty() {
        None
    } else {
        Some(changes)
    }
}

fn push_change(
    changes: &mut Vec<PullChange>,
    id: &str,
    path: &str,
    field: &str,
    local: Option<String>,
    remote: Option<String>,
    overwrite: bool,
) {
    if !should_apply(local.as_deref(), remote.as_deref(), overwrite) {
        return;
    }
    if normalize_value(local.as_deref()) == normalize_value(remote.as_deref()) {
        return;
    }
    changes.push(PullChange {
        id: id.to_string(),
        path: path.to_string(),
        field: format!("fields.{field}"),
        local,
        remote,
    });
}

fn push_body_change(
    changes: &mut Vec<PullChange>,
    id: &str,
    path: &str,
    local: Option<String>,
    remote: Option<String>,
    overwrite: bool,
) {
    if !should_apply(local.as_deref(), remote.as_deref(), overwrite) {
        return;
    }
    if normalize_value(local.as_deref()) == normalize_value(remote.as_deref()) {
        return;
    }
    changes.push(PullChange {
        id: id.to_string(),
        path: path.to_string(),
        field: "body".to_string(),
        local,
        remote,
    });
}

fn push_state_change(
    changes: &mut Vec<PullChange>,
    id: &str,
    path: &str,
    local: Option<String>,
    remote: Option<String>,
) {
    if remote.is_none() {
        return;
    }
    if parse_issue_state(local.as_deref()) == parse_issue_state(remote.as_deref()) {
        return;
    }
    changes.push(PullChange {
        id: id.to_string(),
        path: path.to_string(),
        field: "state".to_string(),
        local,
        remote,
    });
}

fn should_apply(local: Option<&str>, remote: Option<&str>, overwrite: bool) -> bool {
    if overwrite {
        return true;
    }
    normalize_value(local).is_none() && normalize_value(remote).is_some()
}

fn normalize_value(value: Option<&str>) -> Option<&str> {
    value.filter(|raw| !raw.is_empty())
}

fn parse_issue_state(value: Option<&str>) -> Option<IssueState> {
    let state = value?.trim();
    if state.eq_ignore_ascii_case("closed") {
        Some(IssueState::Closed)
    } else if state.eq_ignore_ascii_case("open") {
        Some(IssueState::Open)
    } else {
        None
    }
}

fn issue_state_to_str(state: IssueState) -> &'static str {
    match state {
        IssueState::Open => "open",
        IssueState::Closed => "closed",
    }
}

fn resolve_remote_value(
    mapping: &ProjectFieldMapping,
    issue_url: &str,
    project_items: &HashMap<String, ProjectItemState>,
    issue_fields_by_url: &HashMap<String, HashMap<String, String>>,
) -> Option<String> {
    project_items
        .get(issue_url)
        .and_then(|item| item.field_values.get(&mapping.field_id))
        .cloned()
        .or_else(|| {
            issue_fields_by_url
                .get(issue_url)
                .and_then(|fields| fields.get(&mapping.field_name))
                .cloned()
        })
}

fn find_missing_urls(
    roadmap: &Roadmap,
    config: &RoadmapConfig,
    project_items: &HashMap<String, ProjectItemState>,
) -> (Vec<String>, Vec<String>) {
    let mut local_urls: HashSet<String> = HashSet::new();
    let mut missing_issue_urls = Vec::new();
    let mut missing_milestone_urls = Vec::new();
    let hub_full_repo = config.hub_full_repo().ok();

    for issue in &roadmap.issues {
        if let Some(number) = issue.github_number() {
            if let Ok(full_repo) = config.full_repo(&issue.file.frontmatter.repo) {
                local_urls.insert(format!("https://github.com/{full_repo}/issues/{number}"));
            }
        }
    }
    if let Some(hub_repo) = &hub_full_repo {
        for milestone in &roadmap.milestones {
            if let Some(number) = milestone.github_number() {
                local_urls.insert(format!("https://github.com/{hub_repo}/issues/{number}"));
            }
        }
    }

    for issue_url in project_items.keys() {
        if local_urls.contains(issue_url) {
            continue;
        }
        let Some(parsed) = parse_issue_url(issue_url) else {
            continue;
        };
        if hub_full_repo.as_deref() == Some(parsed.full_repo.as_str()) {
            missing_milestone_urls.push(issue_url.clone());
        } else {
            missing_issue_urls.push(issue_url.clone());
        }
    }

    missing_issue_urls.sort();
    missing_milestone_urls.sort();
    (missing_issue_urls, missing_milestone_urls)
}

pub fn print_pull_plan(plan: &PullPlan, overwrite: bool, import_missing: bool) {
    if plan.is_empty() {
        println!("pull: no changes");
        return;
    }

    println!(
        "pull: {} change(s), {} missing issue(s), {} missing milestone(s)",
        plan.changes.len(),
        plan.missing_issue_urls.len(),
        plan.missing_milestone_urls.len()
    );
    println!(
        "  mode: {}{}",
        if overwrite { "overwrite" } else { "fill-empty-only" },
        if import_missing { " + import-missing" } else { "" }
    );
    println!();

    for change in &plan.changes {
        println!(
            "{} {} — {}: {} → {}",
            style::tag(&change.id),
            change.path,
            style::emphasis(&change.field),
            format_value(change.local.as_deref()),
            format_value(change.remote.as_deref()),
        );
    }

    if !plan.missing_issue_urls.is_empty() {
        println!();
        println!("missing issues:");
        for url in &plan.missing_issue_urls {
            println!("  - {url}");
        }
    }
    if !plan.missing_milestone_urls.is_empty() {
        println!();
        println!("missing milestones:");
        for url in &plan.missing_milestone_urls {
            println!("  - {url}");
        }
    }
}

fn format_value(value: Option<&str>) -> String {
    value.map_or_else(|| "null".to_string(), |v| format!("`{v}`"))
}

pub async fn pull(
    roadmap_dir: &Path,
    platform: &dyn Platform,
    skip_confirm: bool,
    overwrite: bool,
    import_missing: bool,
) -> Result<()> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let roadmap = load(roadmap_dir)?;
    validate_or_err(&roadmap, &config)?;

    let github = config.github()?;
    let project = ProjectRef { owner: github.owner.clone(), number: github.project_number };

    println!("{}", style::heading("pull"));
    let items = platform.list_project_items(&project).await?;
    let project_items: HashMap<String, ProjectItemState> = items
        .into_iter()
        .map(|item| {
            (
                item.issue_url,
                ProjectItemState { item_id: item.item_id, field_values: item.field_values },
            )
        })
        .collect();
    let meta = platform.load_project_meta(&project).await?;
    let issue_fields_by_url = fetch_issue_fields(platform, &project_items).await?;
    let snapshots_by_url = fetch_local_issue_snapshots(&roadmap, &config, platform).await?;
    println!(
        "{} — {} project item(s) with issue links, {} issue field snapshot(s), {} body snapshot(s)",
        style::ok("fetched"),
        project_items.len(),
        issue_fields_by_url.len(),
        snapshots_by_url.len()
    );
    println!();

    let mut sync_map = roadmap.sync_map.clone();
    sync_field_mappings(&mut sync_map, &meta);
    let plan = build_pull_plan(
        &roadmap,
        &config,
        &sync_map,
        &project_items,
        &issue_fields_by_url,
        &snapshots_by_url,
        overwrite,
    );
    print_pull_plan(&plan, overwrite, import_missing);
    if plan.is_empty() {
        return Ok(());
    }

    if !skip_confirm {
        eprintln!();
        eprintln!("pull will apply the changes above. Re-run with --yes to confirm.");
        return Ok(());
    }

    let mut import_report = ImportReport::default();
    if import_missing {
        import_report = import_missing_items(
            roadmap_dir,
            &roadmap,
            &config,
            platform,
            &project_items,
            &issue_fields_by_url,
            &mut sync_map,
        )
        .await?;
    }

    let refreshed_roadmap = load(roadmap_dir)?;
    let refreshed_plan = build_pull_plan(
        &refreshed_roadmap,
        &config,
        &sync_map,
        &project_items,
        &issue_fields_by_url,
        &snapshots_by_url,
        overwrite,
    );
    apply_pull_plan(&refreshed_roadmap, &refreshed_plan)?;
    let sync_map_path = default_sync_map_path(roadmap_dir);
    sync_map.save(&sync_map_path)?;

    println!();
    println!(
        "{} — {} file(s) updated, {} issue(s) imported, {} milestone(s) touched",
        style::ok("pull complete"),
        count_affected_files(&refreshed_plan),
        import_report.created_issues,
        import_report.touched_milestones
    );
    println!("  sync-map saved {}", sync_map_path.display());
    Ok(())
}

fn sync_field_mappings(sync_map: &mut SyncMap, meta: &ProjectMeta) {
    sync_map.meta.fields.issue = meta
        .fields
        .iter()
        .filter(|field| field_key(&field.name) != "title")
        .map(|field| {
            (
                field_key(&field.name),
                ProjectFieldMapping {
                    field_id: field.id.clone(),
                    field_name: field.name.clone(),
                    field_type: field.field_type.clone(),
                },
            )
        })
        .collect();
    sync_map.meta.fields.milestone = sync_map.meta.fields.issue.clone();
}

fn field_key(name: &str) -> String {
    let mut key = String::new();
    let mut last_underscore = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            key.push(ch.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore {
            key.push('_');
            last_underscore = true;
        }
    }
    key.trim_matches('_').to_string()
}

async fn fetch_issue_fields(
    platform: &dyn Platform,
    project_items: &HashMap<String, ProjectItemState>,
) -> Result<HashMap<String, HashMap<String, String>>> {
    let entries: Vec<(String, ParsedIssueUrl)> = project_items
        .keys()
        .filter_map(|url| parse_issue_url(url).map(|parsed| (url.clone(), parsed)))
        .collect();
    let pairs: Vec<(String, HashMap<String, String>)> = stream::iter(entries)
        .map(|(url, parsed)| async move {
            let values = platform
                .get_issue_field_values(&IssueRef { repo: parsed.full_repo, number: parsed.number })
                .await?;
            Ok::<_, crate::error::Error>((url, values))
        })
        .buffer_unordered(8)
        .try_collect()
        .await?;
    Ok(pairs.into_iter().collect())
}

async fn fetch_local_issue_snapshots(
    roadmap: &Roadmap,
    config: &RoadmapConfig,
    platform: &dyn Platform,
) -> Result<HashMap<String, RemoteIssueSnapshot>> {
    let mut repo_numbers: HashMap<String, BTreeSet<u64>> = HashMap::new();

    for issue in &roadmap.issues {
        let Some(number) = issue.github_number() else {
            continue;
        };
        let full_repo = config.full_repo(&issue.file.frontmatter.repo)?;
        repo_numbers.entry(full_repo).or_default().insert(number);
    }

    let hub_full_repo = config.hub_full_repo()?;
    for milestone in &roadmap.milestones {
        let Some(number) = milestone.github_number() else {
            continue;
        };
        repo_numbers.entry(hub_full_repo.clone()).or_default().insert(number);
    }

    let mut snapshots_by_url = HashMap::new();
    for (repo, numbers) in repo_numbers {
        let issue_numbers: Vec<u64> = numbers.into_iter().collect();
        let snapshots = platform.get_issues_snapshot(&repo, &issue_numbers).await?;
        for snapshot in snapshots.into_values() {
            snapshots_by_url.insert(snapshot.issue.url.clone(), snapshot);
        }
    }

    Ok(snapshots_by_url)
}

async fn import_missing_items(
    roadmap_dir: &Path,
    roadmap: &Roadmap,
    config: &RoadmapConfig,
    platform: &dyn Platform,
    project_items: &HashMap<String, ProjectItemState>,
    issue_fields_by_url: &HashMap<String, HashMap<String, String>>,
    sync_map: &mut SyncMap,
) -> Result<ImportReport> {
    let hub_full_repo = config.hub_full_repo()?;
    let issues_dir = roadmap_dir.join("issues");
    let milestones_dir = roadmap_dir.join("milestones");

    let mut report = ImportReport::default();
    let mut used_ids: HashSet<String> = roadmap
        .issues
        .iter()
        .map(|issue| issue.id().to_string())
        .chain(roadmap.milestones.iter().map(|milestone| milestone.id().to_string()))
        .collect();

    let mut milestone_by_number: HashMap<u64, String> = roadmap
        .milestones
        .iter()
        .filter_map(|milestone| {
            milestone.github_number().map(|number| (number, milestone.id().to_string()))
        })
        .collect();
    let existing_milestones: HashMap<u64, &Milestone> = roadmap
        .milestones
        .iter()
        .filter_map(|milestone| milestone.github_number().map(|number| (number, milestone)))
        .collect();

    let mut pending_milestones: HashMap<u64, PendingMilestone> = HashMap::new();
    let mut imported_issue_urls: BTreeSet<String> = BTreeSet::new();

    for issue_url in project_items.keys() {
        let Some(parsed) = parse_issue_url(issue_url) else {
            continue;
        };
        if parsed.full_repo == hub_full_repo {
            continue;
        }
        if issue_exists_locally(roadmap, config, &parsed.full_repo, parsed.number) {
            continue;
        }
        let Some(repo_id) = repo_id_for_full_repo(config, &parsed.full_repo) else {
            continue;
        };

        let remote = platform
            .get_issue(&IssueRef { repo: parsed.full_repo.clone(), number: parsed.number })
            .await?;
        let issue_slug_base = slugify(&remote.title);
        let issue_id = unique_id(&format!("{repo_id}-{issue_slug_base}"), &mut used_ids);
        let issue_slug = issue_id
            .strip_prefix(&format!("{repo_id}-"))
            .unwrap_or(issue_slug_base.as_str())
            .to_string();
        let issue_path = issues_dir.join(format!("{repo_id}-#{}-{issue_slug}.md", parsed.number));

        let mut milestone_id = None;
        if let Some(parent_url) = remote.parent_url.as_deref() {
            if let Some(parent) = parse_issue_url(parent_url) {
                if parent.full_repo == hub_full_repo {
                    let ensured = ensure_milestone(
                        parent.number,
                        &existing_milestones,
                        &mut pending_milestones,
                        &mut milestone_by_number,
                        &mut used_ids,
                        &milestones_dir,
                        platform,
                        &hub_full_repo,
                        project_items,
                        issue_fields_by_url,
                        sync_map,
                    )
                    .await?;
                    milestone_id = Some(ensured.clone());
                    if let Some(pending) = pending_milestones.get_mut(&parent.number) {
                        if !pending.frontmatter.issues.contains(&issue_id) {
                            pending.frontmatter.issues.push(issue_id.clone());
                            pending.frontmatter.issues.sort();
                        }
                    } else if let Some(existing) = existing_milestones.get(&parent.number) {
                        let mut cloned = existing.file.frontmatter.clone();
                        if !cloned.issues.contains(&issue_id) {
                            cloned.issues.push(issue_id.clone());
                            cloned.issues.sort();
                            pending_milestones.insert(
                                parent.number,
                                PendingMilestone {
                                    frontmatter: cloned,
                                    path: existing.file.path.clone(),
                                    body: existing.file.body.clone(),
                                },
                            );
                        }
                    }
                }
            }
        }

        let issue_fields = mapped_fields(
            &sync_map.meta.fields.issue,
            issue_url,
            project_items,
            issue_fields_by_url,
        );
        let issue_frontmatter = IssueFrontmatter {
            id: issue_id.clone(),
            repo: repo_id.clone(),
            title: remote.title,
            url: Some(remote.url.clone()),
            state: map_issue_state(&remote.state),
            labels: vec![],
            milestone: milestone_id,
            blocked_by: vec![],
            fields: issue_fields,
        };
        write_markdown_file(&issue_path, &issue_frontmatter, &remote.body)?;
        sync_map.upsert_issue(
            issue_id,
            IssueSyncEntry {
                repo: parsed.full_repo.clone(),
                issue_number: Some(parsed.number),
                project_item_id: project_items.get(issue_url).map(|item| item.item_id.clone()),
                updated_at: Some(OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default()),
            },
        );
        imported_issue_urls.insert(issue_url.clone());
        report.created_issues += 1;
    }

    for (number, pending) in pending_milestones {
        let issue_url = format!("https://github.com/{hub_full_repo}/issues/{number}");
        write_markdown_file(&pending.path, &pending.frontmatter, &pending.body)?;
        sync_map.upsert_issue(
            pending.frontmatter.id.clone(),
            IssueSyncEntry {
                repo: hub_full_repo.clone(),
                issue_number: Some(number),
                project_item_id: project_items.get(&issue_url).map(|item| item.item_id.clone()),
                updated_at: Some(OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default()),
            },
        );
        report.touched_milestones += 1;
    }

    if !imported_issue_urls.is_empty() {
        println!("{} — imported {} issue(s)", style::ok("import"), imported_issue_urls.len());
    }
    Ok(report)
}

fn mapped_fields(
    mappings: &HashMap<String, ProjectFieldMapping>,
    issue_url: &str,
    project_items: &HashMap<String, ProjectItemState>,
    issue_fields_by_url: &HashMap<String, HashMap<String, String>>,
) -> std::collections::BTreeMap<String, Option<String>> {
    let mut out = std::collections::BTreeMap::new();
    for (key, mapping) in mappings {
        let value = resolve_remote_value(mapping, issue_url, project_items, issue_fields_by_url);
        out.insert(key.clone(), value);
    }
    out
}

async fn ensure_milestone(
    number: u64,
    existing_milestones: &HashMap<u64, &Milestone>,
    pending_milestones: &mut HashMap<u64, PendingMilestone>,
    milestone_by_number: &mut HashMap<u64, String>,
    used_ids: &mut HashSet<String>,
    milestones_dir: &Path,
    platform: &dyn Platform,
    hub_full_repo: &str,
    project_items: &HashMap<String, ProjectItemState>,
    issue_fields_by_url: &HashMap<String, HashMap<String, String>>,
    sync_map: &SyncMap,
) -> Result<String> {
    if let Some(id) = milestone_by_number.get(&number) {
        return Ok(id.clone());
    }
    if let Some(existing) = existing_milestones.get(&number) {
        let id = existing.id().to_string();
        milestone_by_number.insert(number, id.clone());
        return Ok(id);
    }
    if let Some(pending) = pending_milestones.get(&number) {
        return Ok(pending.frontmatter.id.clone());
    }

    let remote = platform.get_issue(&IssueRef { repo: hub_full_repo.to_string(), number }).await?;
    let base_slug = slugify(&remote.title);
    let id = unique_id(&format!("milestone-{base_slug}"), used_ids);
    let slug = id.strip_prefix("milestone-").unwrap_or(base_slug.as_str()).to_string();
    let path = milestones_dir.join(format!("milestone-#{number}-{slug}.md"));
    let issue_url = format!("https://github.com/{hub_full_repo}/issues/{number}");
    let fields = mapped_fields(
        &sync_map.meta.fields.milestone,
        &issue_url,
        project_items,
        issue_fields_by_url,
    );
    let frontmatter = MilestoneFrontmatter {
        id: id.clone(),
        title: remote.title,
        status: MilestoneStatus::Planned,
        issues: Vec::new(),
        fields,
    };
    pending_milestones.insert(number, PendingMilestone { frontmatter, path, body: remote.body });
    milestone_by_number.insert(number, id.clone());
    Ok(id)
}

fn issue_exists_locally(
    roadmap: &Roadmap,
    config: &RoadmapConfig,
    full_repo: &str,
    number: u64,
) -> bool {
    roadmap.issues.iter().any(|issue| {
        issue.github_number() == Some(number)
            && config.full_repo(&issue.file.frontmatter.repo).is_ok_and(|repo| repo == full_repo)
    })
}

fn repo_id_for_full_repo(config: &RoadmapConfig, full_repo: &str) -> Option<String> {
    let github = config.github().ok()?;
    github.repos.iter().find_map(|(repo_id, repo_slug)| {
        if format!("{}/{}", github.owner, repo_slug) == full_repo {
            Some(repo_id.clone())
        } else {
            None
        }
    })
}

fn unique_id(base: &str, used: &mut HashSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    for n in 2.. {
        let candidate = format!("{base}_{n}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("unbounded loop always returns")
}

fn map_issue_state(state: &str) -> IssueState {
    if state.eq_ignore_ascii_case("closed") {
        IssueState::Closed
    } else {
        IssueState::Open
    }
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_underscore = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed
    }
}

fn parse_issue_url(url: &str) -> Option<ParsedIssueUrl> {
    let trimmed = url.trim_end_matches('/');
    let parts: Vec<_> = trimmed.split('/').collect();
    if parts.len() < 2 {
        return None;
    }
    let number = parts.last()?.parse::<u64>().ok()?;
    let issues_idx = parts.iter().position(|part| *part == "issues")?;
    if issues_idx < 2 {
        return None;
    }
    let owner = parts.get(issues_idx - 2)?;
    let repo = parts.get(issues_idx - 1)?;
    Some(ParsedIssueUrl { full_repo: format!("{owner}/{repo}"), number })
}

fn count_affected_files(plan: &PullPlan) -> usize {
    plan.changes.iter().map(|c| &c.path).collect::<BTreeSet<_>>().len()
}

fn apply_pull_plan(roadmap: &Roadmap, plan: &PullPlan) -> Result<()> {
    let issues_by_id: HashMap<&str, &Issue> =
        roadmap.issues.iter().map(|issue| (issue.id(), issue)).collect();
    let milestones_by_id: HashMap<&str, &Milestone> =
        roadmap.milestones.iter().map(|milestone| (milestone.id(), milestone)).collect();

    let mut issue_updates: HashMap<String, IssueFrontmatter> = HashMap::new();
    let mut milestone_updates: HashMap<String, MilestoneFrontmatter> = HashMap::new();
    let mut issue_body_updates: HashMap<String, String> = HashMap::new();
    let mut milestone_body_updates: HashMap<String, String> = HashMap::new();

    for change in &plan.changes {
        if let Some(field_key) = change.field.strip_prefix("fields.") {
            if let Some(issue) = issues_by_id.get(change.id.as_str()) {
                let frontmatter = issue_updates
                    .entry(change.id.clone())
                    .or_insert_with(|| issue.file.frontmatter.clone());
                frontmatter.fields.insert(field_key.to_string(), change.remote.clone());
                continue;
            }
            if let Some(milestone) = milestones_by_id.get(change.id.as_str()) {
                let frontmatter = milestone_updates
                    .entry(change.id.clone())
                    .or_insert_with(|| milestone.file.frontmatter.clone());
                frontmatter.fields.insert(field_key.to_string(), change.remote.clone());
                continue;
            }
            return Err(crate::error::Error::Config(format!(
                "item `{}` not found for pull change",
                change.id
            )));
        }
        if change.field == "body" {
            let body = change.remote.clone().unwrap_or_default();
            if issues_by_id.contains_key(change.id.as_str()) {
                issue_body_updates.insert(change.id.clone(), body);
                continue;
            }
            if milestones_by_id.contains_key(change.id.as_str()) {
                milestone_body_updates.insert(change.id.clone(), body);
                continue;
            }
            return Err(crate::error::Error::Config(format!(
                "item `{}` not found for pull body change",
                change.id
            )));
        }
        if change.field == "state" {
            let Some(state) = parse_issue_state(change.remote.as_deref()) else {
                return Err(crate::error::Error::Config(format!(
                    "unsupported issue state value `{:?}` for `{}`",
                    change.remote, change.id
                )));
            };
            if let Some(issue) = issues_by_id.get(change.id.as_str()) {
                let frontmatter = issue_updates
                    .entry(change.id.clone())
                    .or_insert_with(|| issue.file.frontmatter.clone());
                frontmatter.state = state;
                continue;
            }
            return Err(crate::error::Error::Config(format!(
                "item `{}` not found for pull state change",
                change.id
            )));
        }
        return Err(crate::error::Error::Config(format!(
            "unsupported pull field `{}`",
            change.field
        )));
    }

    let issue_ids: BTreeSet<String> =
        issue_updates.keys().chain(issue_body_updates.keys()).cloned().collect();
    for id in issue_ids {
        let issue = issues_by_id
            .get(id.as_str())
            .ok_or_else(|| crate::error::Error::Config(format!("issue `{id}` not found")))?;
        let frontmatter =
            issue_updates.remove(&id).unwrap_or_else(|| issue.file.frontmatter.clone());
        let body = issue_body_updates.remove(&id).unwrap_or_else(|| issue.file.body.clone());
        write_markdown_file(&issue.file.path, &frontmatter, &body)?;
        println!("  updated {}", issue.file.path.display());
    }

    let milestone_ids: BTreeSet<String> =
        milestone_updates.keys().chain(milestone_body_updates.keys()).cloned().collect();
    for id in milestone_ids {
        let milestone = milestones_by_id
            .get(id.as_str())
            .ok_or_else(|| crate::error::Error::Config(format!("milestone `{id}` not found")))?;
        let frontmatter =
            milestone_updates.remove(&id).unwrap_or_else(|| milestone.file.frontmatter.clone());
        let body =
            milestone_body_updates.remove(&id).unwrap_or_else(|| milestone.file.body.clone());
        write_markdown_file(&milestone.file.path, &frontmatter, &body)?;
        println!("  updated {}", milestone.file.path.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_issue_url() {
        let parsed = parse_issue_url("https://github.com/example/app/issues/40").unwrap();
        assert_eq!(parsed.full_repo, "example/app");
        assert_eq!(parsed.number, 40);
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Epic 1: AI generated dashboard"), "epic_1_ai_generated_dashboard");
    }

    #[test]
    fn parses_issue_state_case_insensitive() {
        assert_eq!(parse_issue_state(Some("open")), Some(IssueState::Open));
        assert_eq!(parse_issue_state(Some("CLOSED")), Some(IssueState::Closed));
    }

    #[test]
    fn ignores_unknown_issue_state() {
        assert_eq!(parse_issue_state(Some("in_progress")), None);
    }
}
