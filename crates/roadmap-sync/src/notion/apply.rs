use std::io::Write;
use std::path::Path;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::config::{default_sync_map_path, RoadmapConfig};
use crate::error::{Error, Result};
use crate::loader::load;
use crate::style;
use crate::validate::validate_or_err;

use super::client::NotionClient;
use super::mapping::build_page_properties;
use super::plan::{build_notion_plan, notion_page_id, print_notion_plan, NotionAction};

pub async fn apply_notion(roadmap_dir: &Path, skip_confirm: bool) -> Result<()> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let notion_cfg = config.notion_config()?;
    let roadmap = load(roadmap_dir)?;
    validate_or_err(&roadmap, &config)?;

    let meta = roadmap.sync_map.meta.notion.as_ref().ok_or_else(|| {
        Error::Config(
            "Notion property mapping missing in sync-map (_meta.notion); run `roadmap setup --notion-only`"
                .into(),
        )
    })?;
    if meta.data_source_id != notion_cfg.data_source_id {
        return Err(Error::Config(format!(
            "sync-map Notion data_source_id `{}` does not match roadmap.toml `{}` (re-run setup --notion-only)",
            meta.data_source_id, notion_cfg.data_source_id
        )));
    }

    NotionClient::ensure_on_path()?;
    let client = NotionClient::new();

    let plan = build_notion_plan(&roadmap)?;
    print_notion_plan(&plan);

    if plan.is_empty() {
        return Ok(());
    }

    if !skip_confirm {
        eprintln!();
        eprintln!("apply will execute the Notion plan above. Re-run with --yes to confirm.");
        return Ok(());
    }

    println!();
    println!("notion apply: {} job(s)", plan.job_count());

    let mut sync_map = roadmap.sync_map.clone();
    let mappings = meta.properties.clone();
    let data_source_id = meta.data_source_id.clone();

    for (idx, job) in plan.jobs.iter().enumerate() {
        let milestone = roadmap
            .milestone_by_id(&job.id)
            .ok_or_else(|| Error::Config(format!("milestone `{}` not found", job.id)))?;
        println!(
            "{} {} ({})",
            style::job_index(idx + 1, plan.job_count()),
            job.id,
            job.action.label()
        );
        println!("  {}", job.title);
        let _ = std::io::stdout().flush();

        let properties = build_page_properties(milestone, &mappings)?;
        let now = OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default();

        match job.action {
            NotionAction::CreatePage => {
                let page_id = client.create_page(&data_source_id, properties).await?;
                sync_map.set_notion_page_id(milestone.id(), page_id.clone(), Some(now));
                println!("      {} — created {page_id}", style::status_ok());
            }
            NotionAction::UpdatePage => {
                let page_id = notion_page_id(&sync_map, milestone.id()).ok_or_else(|| {
                    Error::Config(format!("missing notion_page_id for `{}`", milestone.id()))
                })?;
                client.update_page_properties(&page_id, properties).await?;
                sync_map.set_notion_page_id(milestone.id(), page_id.clone(), Some(now));
                println!("      {} — updated {page_id}", style::status_ok());
            }
        }
        println!();
    }

    let sync_map_path = default_sync_map_path(roadmap_dir);
    sync_map.save(&sync_map_path)?;
    println!(
        "{} — sync-map updated ({})",
        style::ok("notion apply complete"),
        sync_map_path.display()
    );
    Ok(())
}
