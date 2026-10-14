mod apply;
mod body_refs;
mod cli;
mod config;
mod doctor;
mod error;
mod field_sync;
mod loader;
mod model;
mod notion;
mod overview;
mod parse;
mod plan;
mod platform;
mod prefetch;
mod pull;
mod setup;
mod show;
mod style;
mod targets;
mod validate;
mod write;

use std::path::Path;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::config::RoadmapConfig;
use crate::error::Result;
use crate::loader::load;
use crate::model::IssueState;
use crate::overview::OverviewFilter;
use crate::plan::{build_plan, print_plan_with_milestones};
use crate::platform::from_config;
use crate::targets::{format_targets, parse_platform_list, resolve_targets, SyncTarget};
use crate::validate::{print_report, validate, validate_or_err};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("roadmap_sync=info".parse().expect("valid directive")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let roadmap_dir = cli.roadmap_dir;

    match cli.command {
        Command::Validate => cmd_validate(&roadmap_dir),
        Command::Setup { force, no_skill, skill_dir, notion_only } => {
            setup::run(
                &roadmap_dir,
                setup::SetupOptions { force, no_skill, skill_dir, notion_only },
            )
            .await
        }
        Command::Doctor => cmd_doctor(&roadmap_dir).await,
        Command::Plan { offline, platform } => {
            cmd_plan(&roadmap_dir, offline, platform.as_deref()).await
        }
        Command::Apply { yes, platform } => cmd_apply(&roadmap_dir, yes, platform.as_deref()).await,
        Command::Pull { yes, overwrite, import_missing } => {
            cmd_pull(&roadmap_dir, yes, overwrite, import_missing).await
        }
        Command::Show { issue, repo } => cmd_show(&roadmap_dir, issue, repo.as_deref()).await,
        Command::Overview {
            milestone,
            status,
            exclude_status,
            exclude_milestone,
            state,
            repo,
            dated,
            issues_only,
            milestones_only,
            flat,
        } => {
            let state = match state.as_deref() {
                Some("open") => Some(IssueState::Open),
                Some("closed") => Some(IssueState::Closed),
                None => None,
                Some(other) => {
                    return Err(error::Error::Config(format!(
                        "invalid --state `{other}` (expected open or closed)"
                    )));
                }
            };
            overview::overview(
                &roadmap_dir,
                &OverviewFilter {
                    milestones: milestone,
                    statuses: status,
                    exclude_statuses: exclude_status,
                    exclude_milestones: exclude_milestone,
                    state,
                    repo,
                    dated,
                    issues_only,
                    milestones_only,
                    flat,
                },
            )
        }
    }
}

async fn cmd_doctor(roadmap_dir: &Path) -> Result<()> {
    let (report, config) = doctor::run(roadmap_dir).await?;
    doctor::print_report(&report, &config);
    if report.is_ok() {
        Ok(())
    } else {
        Err(error::Error::Auth)
    }
}

fn cmd_validate(roadmap_dir: &Path) -> Result<()> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let roadmap = load(roadmap_dir)?;
    let report = validate(&roadmap, &config);
    print_report(&report);
    if report.is_ok() {
        Ok(())
    } else {
        Err(error::Error::Validation(report.errors.len()))
    }
}

async fn cmd_plan(roadmap_dir: &Path, offline: bool, platform: Option<&str>) -> Result<()> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let roadmap = load(roadmap_dir)?;
    validate_or_err(&roadmap, &config)?;

    let requested = parse_platform_list(platform)?;
    let targets = resolve_targets(&config, requested.as_ref())?;
    println!("platforms: {}", format_targets(&targets));
    println!();

    if targets.contains(&SyncTarget::Github) {
        let cache = if offline {
            None
        } else {
            let platform = from_config(&config)?;
            let cache = prefetch::fetch(&roadmap, &config, platform.as_ref()).await?;
            println!();
            Some(cache)
        };

        let actions = build_plan(&roadmap, &config, cache.as_ref())?;
        print_plan_with_milestones(&actions, &roadmap);
    }

    if targets.contains(&SyncTarget::Notion) {
        if targets.contains(&SyncTarget::Github) {
            println!();
        }
        let notion_plan = notion::build_notion_plan(&roadmap)?;
        notion::print_notion_plan(&notion_plan);
    }

    Ok(())
}

async fn cmd_apply(roadmap_dir: &Path, yes: bool, platform: Option<&str>) -> Result<()> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let requested = parse_platform_list(platform)?;
    let targets = resolve_targets(&config, requested.as_ref())?;
    println!("platforms: {}", format_targets(&targets));
    println!();

    if targets.contains(&SyncTarget::Github) {
        let platform = from_config(&config)?;
        apply::apply(roadmap_dir, platform.as_ref(), yes).await?;
    }

    if targets.contains(&SyncTarget::Notion) {
        if targets.contains(&SyncTarget::Github) {
            println!();
        }
        notion::apply_notion(roadmap_dir, yes).await?;
    }

    Ok(())
}

async fn cmd_pull(
    roadmap_dir: &Path,
    yes: bool,
    overwrite: bool,
    import_missing: bool,
) -> Result<()> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let platform = from_config(&config)?;
    pull::pull(roadmap_dir, platform.as_ref(), yes, overwrite, import_missing).await
}

async fn cmd_show(roadmap_dir: &Path, issue: u64, repo: Option<&str>) -> Result<()> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let platform = from_config(&config)?;
    show::show_issue(roadmap_dir, platform.as_ref(), issue, repo).await
}
