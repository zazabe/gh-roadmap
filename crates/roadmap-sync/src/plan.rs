use std::collections::{BTreeMap, HashMap};

use crate::config::RoadmapConfig;
use crate::error::Result;
use crate::model::{Issue, IssueState, Milestone, ProjectFieldMapping, Roadmap};
use crate::platform::{CreateIssueRequest, IssueFieldDefinition, UpdateIssueRequest};
use crate::prefetch::{self, RemoteCache};
use crate::style;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Milestone,
    Issue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    CreateEpic,
    UpdateEpic,
    CreateIssue,
    UpdateIssue,
    LinkParent,
    AddToProject,
    SetProjectStatus,
    SyncFields,
    RenameFile,
    UpdateSyncMap,
}

impl ActionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::CreateEpic => "CreateEpic",
            Self::UpdateEpic => "UpdateEpic",
            Self::CreateIssue => "CreateIssue",
            Self::UpdateIssue => "UpdateIssue",
            Self::LinkParent => "LinkParent",
            Self::AddToProject => "AddToProject",
            Self::SetProjectStatus => "SetProjectStatus",
            Self::SyncFields => "SyncFields",
            Self::RenameFile => "RenameFile",
            Self::UpdateSyncMap => "UpdateSyncMap",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlanStep {
    pub kind: ActionKind,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct PlanJob {
    pub kind: JobKind,
    pub id: String,
    pub title: String,
    pub repo: String,
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub milestone_jobs: Vec<PlanJob>,
    pub issue_jobs: Vec<PlanJob>,
}

impl Plan {
    pub fn job_count(&self) -> usize {
        self.milestone_jobs.len() + self.issue_jobs.len()
    }

    pub fn step_count(&self) -> usize {
        self.milestone_jobs.iter().chain(self.issue_jobs.iter()).map(|job| job.steps.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.milestone_jobs.is_empty() && self.issue_jobs.is_empty()
    }
}

pub fn build_plan(
    roadmap: &Roadmap,
    config: &RoadmapConfig,
    cache: Option<&RemoteCache>,
) -> Result<Plan> {
    let hub_full_repo = config.hub_full_repo()?;
    let hub_repo_id = config.github()?.hub_repo.clone();

    let mut milestone_jobs = Vec::new();
    for milestone in &roadmap.milestones {
        let steps = milestone_steps(milestone, roadmap, config, cache)?;
        if steps.is_empty() {
            continue;
        }
        milestone_jobs.push(PlanJob {
            kind: JobKind::Milestone,
            id: milestone.id().to_string(),
            title: milestone.file.frontmatter.title.clone(),
            repo: hub_repo_id.clone(),
            steps,
        });
    }

    let mut issue_jobs = Vec::new();
    for issue in &roadmap.issues {
        let steps = issue_steps(issue, roadmap, config, &hub_full_repo, cache)?;
        if steps.is_empty() {
            continue;
        }
        issue_jobs.push(PlanJob {
            kind: JobKind::Issue,
            id: issue.id().to_string(),
            title: issue.file.frontmatter.title.clone(),
            repo: issue.file.frontmatter.repo.clone(),
            steps,
        });
    }

    Ok(Plan { milestone_jobs, issue_jobs })
}

fn milestone_steps(
    milestone: &Milestone,
    roadmap: &Roadmap,
    config: &RoadmapConfig,
    cache: Option<&RemoteCache>,
) -> Result<Vec<PlanStep>> {
    let mut steps = Vec::new();
    let hub_full_repo = config.hub_full_repo()?;
    let project_number = config.github()?.project_number;

    if milestone.is_local_only() {
        steps.push(PlanStep {
            kind: ActionKind::CreateEpic,
            detail: format!("{hub_full_repo}: {}", milestone.file.frontmatter.title),
        });
        steps.push(PlanStep {
            kind: ActionKind::AddToProject,
            detail: format!("project #{project_number}"),
        });
        steps.push(PlanStep { kind: ActionKind::SyncFields, detail: "sync issue fields".into() });
        steps.push(PlanStep {
            kind: ActionKind::RenameFile,
            detail: format!("{} → *-#<new>.md", milestone.file.path.display()),
        });
        steps.push(PlanStep {
            kind: ActionKind::UpdateSyncMap,
            detail: "record epic issue_number and project_item_id".into(),
        });
    } else if let Some(number) = milestone.github_number() {
        let issue_ref = crate::platform::IssueRef { repo: hub_full_repo.clone(), number };
        let needs_update =
            cache.and_then(|cache| cache.issues.get(&issue_ref)).is_none_or(|remote| {
                prefetch::issue_needs_update(
                    &milestone.file.frontmatter.title,
                    &milestone_body(milestone),
                    "open",
                    remote,
                )
            });
        if cache.is_none() || needs_update {
            steps.push(PlanStep {
                kind: ActionKind::UpdateEpic,
                detail: format!("{hub_full_repo}#{number}: {}", milestone.file.frontmatter.title),
            });
        }
        let needs_field_sync = cache.is_none_or(|cache| {
            let remote_fields = cache.issue_fields.get(&issue_ref);
            let field_defs = cache.issue_field_defs_by_repo.get(&hub_full_repo);
            fields_need_sync(
                &milestone.file.frontmatter.fields,
                &roadmap.sync_map.meta.fields.milestone,
                remote_fields,
                field_defs,
            )
        });
        if needs_field_sync {
            steps.push(PlanStep {
                kind: ActionKind::SyncFields,
                detail: "sync issue fields".into(),
            });
        }
        if roadmap.sync_map.issue_entry(milestone.id()).is_none() {
            steps.push(PlanStep {
                kind: ActionKind::UpdateSyncMap,
                detail: "register in sync-map.json".into(),
            });
        }
        let project_item_id = roadmap
            .sync_map
            .issue_entry(milestone.id())
            .and_then(|entry| entry.project_item_id.clone())
            .or_else(|| {
                cache.and_then(|cache| {
                    let issue_url = format!("https://github.com/{hub_full_repo}/issues/{number}");
                    cache.project_items.get(&issue_url).map(|item| item.item_id.clone())
                })
            });
        if project_item_id.is_none() {
            steps.push(PlanStep {
                kind: ActionKind::AddToProject,
                detail: format!("project #{project_number}"),
            });
        } else if cache.is_some()
            && roadmap.sync_map.issue_entry(milestone.id()).is_some()
            && roadmap
                .sync_map
                .issue_entry(milestone.id())
                .and_then(|entry| entry.project_item_id.as_ref())
                .is_none()
        {
            steps.push(PlanStep {
                kind: ActionKind::UpdateSyncMap,
                detail: "record project_item_id from remote".into(),
            });
        }
    }

    Ok(steps)
}

fn issue_steps(
    issue: &Issue,
    roadmap: &Roadmap,
    config: &RoadmapConfig,
    hub_full_repo: &str,
    cache: Option<&RemoteCache>,
) -> Result<Vec<PlanStep>> {
    let mut steps = Vec::new();
    let full_repo = config.full_repo(&issue.file.frontmatter.repo)?;
    let project_number = config.github()?.project_number;
    let parent_detail = issue
        .file
        .frontmatter
        .milestone
        .as_ref()
        .and_then(|milestone_id| epic_url_for_milestone(milestone_id, hub_full_repo, roadmap));

    if issue.is_local_only() {
        steps.push(PlanStep {
            kind: ActionKind::CreateIssue,
            detail: format!("{full_repo}: {}", issue.file.frontmatter.title),
        });
        if let Some(parent) = parent_detail {
            steps.push(PlanStep { kind: ActionKind::LinkParent, detail: parent });
        }
        steps.push(PlanStep {
            kind: ActionKind::AddToProject,
            detail: format!("project #{project_number}"),
        });
        steps.push(PlanStep {
            kind: ActionKind::SetProjectStatus,
            detail: format!("status: {}", issue.project_status()),
        });
        steps.push(PlanStep { kind: ActionKind::SyncFields, detail: "sync issue fields".into() });
        steps.push(PlanStep {
            kind: ActionKind::RenameFile,
            detail: format!("{} → *-#<new>.md", issue.file.path.display()),
        });
        steps.push(PlanStep {
            kind: ActionKind::UpdateSyncMap,
            detail: "record issue_number and project_item_id".into(),
        });
    } else if let Some(number) = issue.github_number() {
        let issue_ref = crate::platform::IssueRef { repo: full_repo.clone(), number };
        let remote_issue = cache.and_then(|cache| cache.issues.get(&issue_ref));
        let needs_update = remote_issue.is_none_or(|remote| {
            prefetch::issue_needs_update(
                &issue.file.frontmatter.title,
                &issue_body(issue),
                &issue_state(issue),
                remote,
            )
        });
        if cache.is_none() || needs_update {
            steps.push(PlanStep {
                kind: ActionKind::UpdateIssue,
                detail: format!("{full_repo}#{number}: {}", issue.file.frontmatter.title),
            });
        }
        if let Some(parent) = parent_detail.clone() {
            let parent_linked =
                remote_issue.is_some_and(|remote| prefetch::parent_already_linked(remote, &parent));
            if cache.is_none() || !parent_linked {
                steps.push(PlanStep { kind: ActionKind::LinkParent, detail: parent });
            }
        }
        if roadmap.sync_map.issue_entry(issue.id()).is_none() {
            steps.push(PlanStep {
                kind: ActionKind::UpdateSyncMap,
                detail: "register in sync-map.json".into(),
            });
        }
        let issue_url = format!("https://github.com/{full_repo}/issues/{number}");
        let project_item =
            cache.and_then(|cache| prefetch::project_item_for_url(cache, &issue_url));
        let project_item_id = roadmap
            .sync_map
            .issue_entry(issue.id())
            .and_then(|entry| entry.project_item_id.clone())
            .or_else(|| project_item.map(|item| item.item_id.clone()));
        let needs_project_add = project_item_id.is_none();
        if needs_project_add {
            steps.push(PlanStep {
                kind: ActionKind::AddToProject,
                detail: format!("project #{project_number}"),
            });
            steps.push(PlanStep {
                kind: ActionKind::SetProjectStatus,
                detail: format!("status: {}", issue.project_status()),
            });
            steps.push(PlanStep {
                kind: ActionKind::SyncFields,
                detail: "sync issue fields".into(),
            });
        } else {
            let status_field_id = cache
                .and_then(|cache| cache.project_meta.as_ref())
                .and_then(|meta| meta.field_by_name("Status"))
                .map(|field| field.id.as_str());
            let needs_status_update = project_item.is_none_or(|item| match status_field_id {
                Some(field_id) => {
                    !prefetch::project_status_matches(item, field_id, issue.project_status())
                }
                None => true,
            });
            if cache.is_none() || needs_status_update {
                steps.push(PlanStep {
                    kind: ActionKind::SetProjectStatus,
                    detail: format!("status: {}", issue.project_status()),
                });
            }
            let needs_field_sync = cache.is_none_or(|cache| {
                let remote_fields = cache.issue_fields.get(&issue_ref);
                let field_defs = cache.issue_field_defs_by_repo.get(&full_repo);
                fields_need_sync(
                    &issue.file.frontmatter.fields,
                    &roadmap.sync_map.meta.fields.issue,
                    remote_fields,
                    field_defs,
                )
            });
            if needs_field_sync {
                steps.push(PlanStep {
                    kind: ActionKind::SyncFields,
                    detail: "sync issue fields".into(),
                });
            }
            if cache.is_some()
                && roadmap.sync_map.issue_entry(issue.id()).is_some()
                && roadmap
                    .sync_map
                    .issue_entry(issue.id())
                    .and_then(|entry| entry.project_item_id.as_ref())
                    .is_none()
            {
                steps.push(PlanStep {
                    kind: ActionKind::UpdateSyncMap,
                    detail: "record project_item_id from remote".into(),
                });
            }
        }
    }

    Ok(steps)
}

fn epic_url_for_milestone(
    milestone_id: &str,
    hub_full_repo: &str,
    roadmap: &Roadmap,
) -> Option<String> {
    roadmap.sync_map.epic_url(milestone_id, hub_full_repo).or_else(|| {
        roadmap.milestone_by_id(milestone_id).and_then(|milestone| {
            milestone
                .github_number()
                .map(|number| format!("https://github.com/{hub_full_repo}/issues/{number}"))
        })
    })
}

fn fields_need_sync(
    local_fields: &std::collections::BTreeMap<String, Option<String>>,
    mappings: &HashMap<String, ProjectFieldMapping>,
    remote_fields: Option<&HashMap<String, String>>,
    field_defs: Option<&Vec<IssueFieldDefinition>>,
) -> bool {
    local_fields.iter().any(|(local_key, local_value)| {
        let field_name =
            mappings.get(local_key).map(|mapping| mapping.field_name.as_str()).or_else(|| {
                field_defs.and_then(|defs| {
                    defs.iter()
                        .find(|field| field_key(&field.name) == *local_key)
                        .map(|field| field.name.as_str())
                })
            });
        let Some(field_name) = field_name else {
            return false;
        };
        let remote_value = remote_fields.and_then(|fields| fields.get(field_name));
        normalize_field_value(local_value.as_deref())
            != normalize_field_value(remote_value.map(String::as_str))
    })
}

fn normalize_field_value(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.is_empty())
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

/// Group jobs under milestones for display (issues without milestone go to "unassigned").
pub fn print_plan_with_milestones(plan: &Plan, roadmap: &Roadmap) {
    if plan.is_empty() {
        println!("plan: no jobs");
        return;
    }

    let epic_by_id: BTreeMap<_, _> =
        plan.milestone_jobs.iter().map(|job| (job.id.as_str(), job)).collect();

    let mut by_milestone: BTreeMap<String, Vec<&PlanJob>> = BTreeMap::new();
    for job in &plan.issue_jobs {
        let milestone = roadmap
            .issues
            .iter()
            .find(|issue| issue.id() == job.id)
            .and_then(|issue| issue.file.frontmatter.milestone.clone())
            .unwrap_or_else(|| "unassigned".into());
        by_milestone.entry(milestone).or_default().push(job);
    }

    let milestone_groups = by_milestone.len();
    println!(
        "plan: {} job(s) ({} epic, {} issue), {} step(s), {} milestone group(s)",
        plan.job_count(),
        plan.milestone_jobs.len(),
        plan.issue_jobs.len(),
        plan.step_count(),
        milestone_groups
    );
    println!();

    let mut job_counter = 0usize;
    let total_jobs = plan.job_count();

    let mut printed_milestones = std::collections::BTreeSet::new();
    for (milestone_id, jobs) in &by_milestone {
        if milestone_id != "unassigned" {
            printed_milestones.insert(milestone_id.clone());
            print_milestone_group(
                milestone_id,
                epic_by_id.get(milestone_id.as_str()).copied(),
                jobs,
                roadmap,
                &mut job_counter,
                total_jobs,
            );
        }
    }

    for milestone in &roadmap.milestones {
        if printed_milestones.contains(milestone.id()) {
            continue;
        }
        if let Some(epic_job) = epic_by_id.get(milestone.id()) {
            print_milestone_group(
                milestone.id(),
                Some(epic_job),
                &[],
                roadmap,
                &mut job_counter,
                total_jobs,
            );
        }
    }

    if let Some(jobs) = by_milestone.get("unassigned") {
        if !jobs.is_empty() {
            println!("{} {}", style::heading("milestone:"), style::heading("unassigned"));
            for job in jobs {
                job_counter += 1;
                print_job_header(job_counter, total_jobs, job);
                print_job_steps(job);
            }
        }
    }
}

fn print_milestone_group(
    milestone_id: &str,
    epic_job: Option<&PlanJob>,
    issue_jobs: &[&PlanJob],
    roadmap: &Roadmap,
    job_counter: &mut usize,
    total_jobs: usize,
) {
    let milestone_title = roadmap
        .milestone_by_id(milestone_id)
        .map_or(milestone_id, |m| m.file.frontmatter.title.as_str());
    println!(
        "{} {}",
        style::heading("milestone:"),
        style::heading(format!("{milestone_title} ({milestone_id})"))
    );

    if let Some(job) = epic_job {
        *job_counter += 1;
        print_job_header(*job_counter, total_jobs, job);
        print_job_steps(job);
    }

    for job in issue_jobs {
        *job_counter += 1;
        print_job_header(*job_counter, total_jobs, job);
        print_job_steps(job);
    }
}

fn print_job_steps(job: &PlanJob) {
    for (step_index, step) in job.steps.iter().enumerate() {
        let prefix = if step_index + 1 == job.steps.len() { "└─" } else { "├─" };
        println!("  {prefix} {}: {}", style::emphasis(step.kind.label()), step.detail);
    }
    println!();
}

fn print_job_header(index: usize, total: usize, job: &PlanJob) {
    let label = match job.kind {
        JobKind::Milestone => "epic",
        JobKind::Issue => "issue",
    };
    println!("{} {label} {} ({})", style::job_index(index, total), job.id, job.repo);
    println!("  {}", job.title);
}

pub(crate) fn issue_body(issue: &Issue) -> String {
    issue.file.body.clone()
}

pub(crate) fn milestone_body(milestone: &Milestone) -> String {
    milestone.file.body.clone()
}

pub(crate) fn issue_state(issue: &Issue) -> String {
    match issue.file.frontmatter.state {
        IssueState::Open => "open".to_string(),
        IssueState::Closed => "closed".to_string(),
    }
}

pub(crate) fn create_request(issue: &Issue, full_repo: &str) -> CreateIssueRequest {
    CreateIssueRequest {
        repo: full_repo.to_string(),
        title: issue.file.frontmatter.title.clone(),
        body: issue_body(issue),
        labels: issue.file.frontmatter.labels.clone(),
        parent_url: None,
    }
}

pub(crate) fn create_epic_request(
    milestone: &Milestone,
    hub_full_repo: &str,
) -> CreateIssueRequest {
    CreateIssueRequest {
        repo: hub_full_repo.to_string(),
        title: milestone.file.frontmatter.title.clone(),
        body: milestone_body(milestone),
        labels: vec!["roadmap".into(), "epic".into()],
        parent_url: None,
    }
}

pub(crate) fn update_request(issue: &Issue, full_repo: &str, number: u64) -> UpdateIssueRequest {
    UpdateIssueRequest {
        repo: full_repo.to_string(),
        number,
        title: issue.file.frontmatter.title.clone(),
        body: issue_body(issue),
        labels: issue.file.frontmatter.labels.clone(),
        state: Some(issue_state(issue)),
    }
}

pub(crate) fn update_epic_request(
    milestone: &Milestone,
    hub_full_repo: &str,
    number: u64,
) -> UpdateIssueRequest {
    UpdateIssueRequest {
        repo: hub_full_repo.to_string(),
        number,
        title: milestone.file.frontmatter.title.clone(),
        body: milestone_body(milestone),
        labels: vec!["roadmap".into(), "epic".into()],
        state: Some("open".to_string()),
    }
}
