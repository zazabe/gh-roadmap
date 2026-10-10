mod apply;
mod body_refs;
mod cli;
mod config;
mod doctor;
mod error;
mod loader;
mod model;
mod parse;
mod plan;
mod platform;
mod prefetch;
mod pull;
mod setup;
mod style;
mod validate;
mod write;

use std::path::Path;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::config::RoadmapConfig;
use crate::error::Result;
use crate::loader::load;
use crate::plan::{build_plan, print_plan_with_milestones};
use crate::platform::from_config;
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
        Command::Setup { force, no_skill, skill_dir } => {
            setup::run(&roadmap_dir, setup::SetupOptions { force, no_skill, skill_dir }).await
        }
        Command::Doctor => cmd_doctor(&roadmap_dir).await,
        Command::Plan { offline } => cmd_plan(&roadmap_dir, offline).await,
        Command::Apply { yes } => cmd_apply(&roadmap_dir, yes).await,
        Command::Pull { yes, overwrite, import_missing } => {
            cmd_pull(&roadmap_dir, yes, overwrite, import_missing).await
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

async fn cmd_plan(roadmap_dir: &Path, offline: bool) -> Result<()> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let roadmap = load(roadmap_dir)?;
    validate_or_err(&roadmap, &config)?;

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
    Ok(())
}

async fn cmd_apply(roadmap_dir: &Path, yes: bool) -> Result<()> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let platform = from_config(&config)?;
    apply::apply(roadmap_dir, platform.as_ref(), yes).await
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
