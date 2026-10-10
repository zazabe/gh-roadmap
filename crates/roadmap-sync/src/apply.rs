use std::io::Write;
use std::path::Path;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::body_refs::BodyRefResolver;
use crate::config::{default_sync_map_path, RoadmapConfig};
use crate::error::Result;
use crate::loader::load;
use crate::model::{IssueSyncEntry, Milestone, ProjectFieldMapping, SyncMap};
use crate::plan::{
    build_plan, create_epic_request, create_request, issue_state, print_plan_with_milestones,
    update_epic_request, update_request, ActionKind, JobKind, PlanJob,
};
use crate::platform::{
    IssueFieldValueUpdate, IssueRef, Platform, ProjectRef, RemoteIssue, SetProjectStatusRequest,
    UpdateIssueRequest,
};
use crate::prefetch::RemoteCache;
use crate::style;
use crate::validate::validate_or_err;
use crate::write::write_markdown_file;

struct JobContext {
    issue_url: Option<String>,
    issue_number: Option<u64>,
    project_item_id: Option<String>,
}

impl JobContext {
    fn record_remote(&mut self, remote: &RemoteIssue) {
        self.issue_url = Some(remote.url.clone());
        self.issue_number = Some(remote.number);
    }
}

fn print_step(step_index: usize, step_total: usize, kind: &str, detail: &str) {
    println!("  [{step_index}/{step_total}] {}: {detail}", style::emphasis(kind));
    let _ = std::io::stdout().flush();
}

fn print_step_ok(result: &str) {
    println!("      {} — {result}", style::status_ok());
}

pub async fn apply(roadmap_dir: &Path, platform: &dyn Platform, skip_confirm: bool) -> Result<()> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let roadmap = load(roadmap_dir)?;
    validate_or_err(&roadmap, &config)?;
    let mut body_refs = BodyRefResolver::from_roadmap(&roadmap, &config)?;

    let cache = crate::prefetch::fetch(&roadmap, &config, platform).await?;
    println!();

    let plan = build_plan(&roadmap, &config, Some(&cache))?;
    print_plan_with_milestones(&plan, &roadmap);

    if plan.is_empty() {
        return Ok(());
    }

    if !skip_confirm {
        eprintln!();
        eprintln!("apply will execute the plan above. Re-run with --yes to confirm.");
        return Ok(());
    }

    println!();
    println!(
        "apply: {} job(s) ({} epic, {} issue), {} step(s)",
        plan.job_count(),
        plan.milestone_jobs.len(),
        plan.issue_jobs.len(),
        plan.step_count()
    );

    let mut sync_map = roadmap.sync_map.clone();
    let hub_full_repo = config.hub_full_repo()?;
    let project = ProjectRef {
        owner: config.github()?.owner.clone(),
        number: config.github()?.project_number,
    };

    let mut job_counter = 0usize;
    let total_jobs = plan.job_count();

    for job in &plan.milestone_jobs {
        job_counter += 1;
        let milestone = roadmap.milestone_by_id(&job.id).ok_or_else(|| {
            crate::error::Error::Config(format!("milestone `{}` not found", job.id))
        })?;

        println!("{} epic {} ({})", style::job_index(job_counter, total_jobs), job.id, job.repo);
        println!("  {}", job.title);

        execute_milestone_job(
            platform,
            &project,
            milestone,
            job,
            &hub_full_repo,
            &mut body_refs,
            &mut sync_map,
            &cache,
        )
        .await?;
        println!();
    }

    for job in &plan.issue_jobs {
        job_counter += 1;
        let issue = roadmap
            .issue_by_id(&job.id)
            .ok_or_else(|| crate::error::Error::Config(format!("issue `{}` not found", job.id)))?;
        let full_repo = config.full_repo(&issue.file.frontmatter.repo)?;

        println!("{} issue {} ({})", style::job_index(job_counter, total_jobs), job.id, job.repo);
        println!("  {}", job.title);

        execute_issue_job(
            platform,
            &project,
            issue,
            job,
            &full_repo,
            &hub_full_repo,
            &mut body_refs,
            &mut sync_map,
            &cache,
        )
        .await?;
        println!();
    }

    normalize_local_refs_after_apply(roadmap_dir, &config, platform).await?;

    let sync_map_path = default_sync_map_path(roadmap_dir);
    sync_map.save(&sync_map_path)?;
    println!("{} — sync-map updated ({})", style::ok("apply complete"), sync_map_path.display());
    Ok(())
}

async fn execute_milestone_job(
    platform: &dyn Platform,
    project: &ProjectRef,
    milestone: &Milestone,
    job: &PlanJob,
    hub_full_repo: &str,
    body_refs: &mut BodyRefResolver,
    sync_map: &mut SyncMap,
    cache: &RemoteCache,
) -> Result<()> {
    let issue_url = milestone
        .github_number()
        .map(|number| format!("https://github.com/{hub_full_repo}/issues/{number}"));
    let mut ctx = JobContext {
        issue_url: issue_url.clone(),
        issue_number: milestone.github_number(),
        project_item_id: sync_map
            .issue_entry(milestone.id())
            .and_then(|entry| entry.project_item_id.clone())
            .or_else(|| {
                issue_url
                    .as_deref()
                    .and_then(|url| cache.project_items.get(url).map(|item| item.item_id.clone()))
            }),
    };

    for (step_index, step) in job.steps.iter().enumerate() {
        let n = step_index + 1;
        let total = job.steps.len();
        print_step(n, total, step.kind.label(), &step.detail);

        match step.kind {
            ActionKind::CreateEpic => {
                let resolved = body_refs.resolve(&milestone.file.body);
                let mut request = create_epic_request(milestone, hub_full_repo);
                request.body = resolved.body;
                let remote = platform.create_issue(request).await?;
                print_step_ok(&format!("{hub_full_repo}#{}", remote.number));
                ctx.record_remote(&remote);
                body_refs.insert(milestone.id(), format!("{hub_full_repo}#{}", remote.number));
            }
            ActionKind::UpdateEpic => {
                let number = milestone.github_number().ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "milestone `{}` has no github number",
                        milestone.id()
                    ))
                })?;
                let resolved = body_refs.resolve(&milestone.file.body);
                let mut request = update_epic_request(milestone, hub_full_repo, number);
                request.body = resolved.body;
                let remote = platform.update_issue(request).await?;
                print_step_ok(&format!("{hub_full_repo}#{number}"));
                ctx.record_remote(&remote);
                body_refs.insert(milestone.id(), format!("{hub_full_repo}#{number}"));
            }
            ActionKind::AddToProject => {
                let issue_url = ctx.issue_url.as_deref().ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "cannot add milestone `{}` to project: missing issue URL",
                        milestone.id()
                    ))
                })?;
                let item = platform.add_issue_to_project(project, issue_url).await?;
                ctx.project_item_id = Some(item.id.clone());
                print_step_ok(&format!("item {}", item.id));
            }
            ActionKind::SyncFields => {
                let number = ctx.issue_number.ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "cannot sync fields for milestone `{}`: missing github number",
                        milestone.id()
                    ))
                })?;
                let updated = sync_issue_fields(
                    platform,
                    &IssueRef { repo: hub_full_repo.to_string(), number },
                    &milestone.file.frontmatter.fields,
                    &sync_map.meta.fields.milestone,
                    cache,
                )
                .await?;
                print_step_ok(&format!("{updated} field(s)"));
            }
            ActionKind::RenameFile => {
                let number = ctx.issue_number.ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "cannot rename milestone `{}`: missing github number",
                        milestone.id()
                    ))
                })?;
                let new_path = crate::loader::linked_issue_path(&milestone.file.path, number);
                std::fs::rename(&milestone.file.path, &new_path)?;
                let resolved = body_refs.resolve(&milestone.file.body);
                write_markdown_file(&new_path, &milestone.file.frontmatter, &resolved.body)?;
                print_step_ok(&new_path.display().to_string());
            }
            ActionKind::UpdateSyncMap => {
                sync_map.upsert_issue(
                    milestone.id(),
                    IssueSyncEntry {
                        repo: hub_full_repo.to_string(),
                        issue_number: ctx.issue_number,
                        project_item_id: ctx.project_item_id.clone(),
                        updated_at: Some(
                            OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default(),
                        ),
                    },
                );
                print_step_ok(milestone.id());
            }
            other => {
                return Err(crate::error::Error::Config(format!(
                    "unexpected action `{other:?}` in milestone job"
                )));
            }
        }
    }

    Ok(())
}

async fn execute_issue_job(
    platform: &dyn Platform,
    project: &ProjectRef,
    issue: &crate::model::Issue,
    job: &PlanJob,
    full_repo: &str,
    hub_full_repo: &str,
    body_refs: &mut BodyRefResolver,
    sync_map: &mut SyncMap,
    cache: &RemoteCache,
) -> Result<()> {
    debug_assert_eq!(job.kind, JobKind::Issue);

    let issue_url = issue
        .github_number()
        .map(|number| format!("https://github.com/{full_repo}/issues/{number}"));
    let mut ctx = JobContext {
        issue_url: issue_url.clone(),
        issue_number: issue.github_number(),
        project_item_id: sync_map
            .issue_entry(issue.id())
            .and_then(|entry| entry.project_item_id.clone())
            .or_else(|| {
                issue_url
                    .as_deref()
                    .and_then(|url| cache.project_items.get(url).map(|item| item.item_id.clone()))
            }),
    };

    for (step_index, step) in job.steps.iter().enumerate() {
        let n = step_index + 1;
        let total = job.steps.len();
        print_step(n, total, step.kind.label(), &step.detail);

        match step.kind {
            ActionKind::CreateIssue => {
                let resolved = body_refs.resolve(&issue.file.body);
                let mut request = create_request(issue, full_repo);
                request.body = resolved.body;
                let remote = platform.create_issue(request).await?;
                print_step_ok(&format!("{full_repo}#{}", remote.number));
                ctx.record_remote(&remote);
                body_refs.insert(issue.id(), format!("{full_repo}#{}", remote.number));
            }
            ActionKind::UpdateIssue => {
                let number = issue.github_number().ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "issue `{}` has no github number",
                        issue.id()
                    ))
                })?;
                let resolved = body_refs.resolve(&issue.file.body);
                let mut request = update_request(issue, full_repo, number);
                request.body = resolved.body;
                let remote = platform.update_issue(request).await?;
                print_step_ok(&format!("{full_repo}#{number}"));
                ctx.record_remote(&remote);
                body_refs.insert(issue.id(), format!("{full_repo}#{number}"));
            }
            ActionKind::LinkParent => {
                let milestone_id = issue.file.frontmatter.milestone.as_ref().ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "issue `{}` has LinkParent step but no milestone",
                        issue.id()
                    ))
                })?;
                let parent_url =
                    sync_map.epic_url(milestone_id, hub_full_repo).ok_or_else(|| {
                        crate::error::Error::Config(format!(
                        "milestone `{milestone_id}` has no synced epic in hub — run epic sync first"
                    ))
                    })?;
                let number = ctx.issue_number.ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "cannot link parent for issue `{}`: missing github number",
                        issue.id()
                    ))
                })?;
                platform.link_parent(full_repo, number, &parent_url).await?;
                print_step_ok(&parent_url);
            }
            ActionKind::AddToProject => {
                let issue_url = ctx.issue_url.as_deref().ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "cannot add issue `{}` to project: missing issue URL",
                        issue.id()
                    ))
                })?;
                let item = platform.add_issue_to_project(project, issue_url).await?;
                ctx.project_item_id = Some(item.id.clone());
                print_step_ok(&format!("item {}", item.id));
            }
            ActionKind::SetProjectStatus => {
                let item_id = ctx.project_item_id.as_deref().ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "cannot set project status for issue `{}`: missing project item id",
                        issue.id()
                    ))
                })?;
                let request = SetProjectStatusRequest {
                    item_id: item_id.to_string(),
                    status: issue.project_status().to_string(),
                };
                if let Some(meta) = cache.project_meta.as_ref() {
                    platform.set_project_item_status_with_meta(project, request, meta).await?;
                } else {
                    platform.set_project_item_status(project, request).await?;
                }
                print_step_ok(issue.project_status());
            }
            ActionKind::SyncFields => {
                let number = ctx.issue_number.ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "cannot sync fields for issue `{}`: missing github number",
                        issue.id()
                    ))
                })?;
                let updated = sync_issue_fields(
                    platform,
                    &IssueRef { repo: full_repo.to_string(), number },
                    &issue.file.frontmatter.fields,
                    &sync_map.meta.fields.issue,
                    cache,
                )
                .await?;
                print_step_ok(&format!("{updated} field(s)"));
            }
            ActionKind::RenameFile => {
                let number = ctx.issue_number.ok_or_else(|| {
                    crate::error::Error::Config(format!(
                        "cannot rename issue `{}`: missing github number",
                        issue.id()
                    ))
                })?;
                let new_path = crate::loader::linked_issue_path(&issue.file.path, number);
                std::fs::rename(&issue.file.path, &new_path)?;
                let mut frontmatter = issue.file.frontmatter.clone();
                frontmatter.url = ctx.issue_url.clone();
                let resolved = body_refs.resolve(&issue.file.body);
                write_markdown_file(&new_path, &frontmatter, &resolved.body)?;
                print_step_ok(&new_path.display().to_string());
            }
            ActionKind::UpdateSyncMap => {
                sync_map.upsert_issue(
                    issue.id(),
                    IssueSyncEntry {
                        repo: full_repo.to_string(),
                        issue_number: ctx.issue_number,
                        project_item_id: ctx.project_item_id.clone(),
                        updated_at: Some(
                            OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default(),
                        ),
                    },
                );
                print_step_ok(issue.id());
            }
            other => {
                return Err(crate::error::Error::Config(format!(
                    "unexpected action `{other:?}` in issue job"
                )));
            }
        }
    }

    Ok(())
}

async fn normalize_local_refs_after_apply(
    roadmap_dir: &Path,
    config: &RoadmapConfig,
    platform: &dyn Platform,
) -> Result<()> {
    let refreshed = load(roadmap_dir)?;
    let mut refs = BodyRefResolver::from_roadmap(&refreshed, config)?;

    let hub_full_repo = config.hub_full_repo()?;
    for milestone in &refreshed.milestones {
        let resolved = refs.resolve(&milestone.file.body);
        if resolved.body == milestone.file.body {
            continue;
        }

        write_markdown_file(&milestone.file.path, &milestone.file.frontmatter, &resolved.body)?;
        if let Some(number) = milestone.github_number() {
            platform
                .update_issue(UpdateIssueRequest {
                    repo: hub_full_repo.clone(),
                    number,
                    title: milestone.file.frontmatter.title.clone(),
                    body: resolved.body.clone(),
                    labels: vec!["roadmap".into(), "epic".into()],
                    state: Some("open".to_string()),
                })
                .await?;
            refs.insert(milestone.id(), format!("{hub_full_repo}#{number}"));
        }
    }

    for issue in &refreshed.issues {
        let resolved = refs.resolve(&issue.file.body);
        if resolved.body == issue.file.body {
            continue;
        }

        write_markdown_file(&issue.file.path, &issue.file.frontmatter, &resolved.body)?;
        if let Some(number) = issue.github_number() {
            let full_repo = config.full_repo(&issue.file.frontmatter.repo)?;
            platform
                .update_issue(UpdateIssueRequest {
                        repo: full_repo.clone(),
                    number,
                    title: issue.file.frontmatter.title.clone(),
                    body: resolved.body.clone(),
                    labels: issue.file.frontmatter.labels.clone(),
                    state: Some(issue_state(issue)),
                })
                .await?;
            refs.insert(issue.id(), format!("{full_repo}#{number}"));
        }
    }

    Ok(())
}

async fn sync_issue_fields(
    platform: &dyn Platform,
    issue_ref: &IssueRef,
    local_fields: &std::collections::BTreeMap<String, Option<String>>,
    mappings: &std::collections::HashMap<String, ProjectFieldMapping>,
    cache: &RemoteCache,
) -> Result<usize> {
    let issue_defs =
        cache.issue_field_defs_by_repo.get(&issue_ref.repo).cloned().unwrap_or_default();
    let remote_values = cache.issue_fields.get(issue_ref);

    let mut updates = Vec::new();
    for (local_key, local_value) in local_fields {
        let field_name =
            mappings.get(local_key).map(|mapping| mapping.field_name.clone()).or_else(|| {
                issue_defs
                    .iter()
                    .find(|field| field_key(&field.name) == *local_key)
                    .map(|field| field.name.clone())
            });
        let Some(field_name) = field_name else {
            continue;
        };
        let local_norm = normalize_field_value(local_value.as_deref());
        let remote_norm = remote_values
            .and_then(|fields| fields.get(&field_name))
            .and_then(|value| normalize_field_value(Some(value.as_str())));
        if local_norm != remote_norm {
            updates
                .push(IssueFieldValueUpdate { field_name, value: local_norm.map(str::to_string) });
        }
    }
    if updates.is_empty() {
        return Ok(0);
    }
    platform.set_issue_field_values(issue_ref, &updates, issue_defs.as_slice()).await?;
    Ok(updates.len())
}

fn normalize_field_value(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
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
