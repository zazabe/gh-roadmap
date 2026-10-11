//! Interactive Notion configuration during `roadmap setup`.

use std::path::Path;

use crate::config::{default_sync_map_path, NotionConfig};
use crate::error::{Error, Result};
use crate::model::{
    MilestoneStatus, NotionPropertyMappings, NotionRelationMapping, NotionStatusMapping,
    NotionSyncMeta, SyncMap,
};
use crate::setup::{prompt_index, prompt_line, prompt_yes_no};
use crate::style;

use super::client::{
    parse_data_source_properties, run_ntn_interactive, NotionClient, NotionDbProperty,
};
use super::mapping::{match_status_values, property_ref};

pub struct NotionSetupResult {
    pub config: NotionConfig,
}

pub async fn configure_notion(roadmap_dir: &Path) -> Result<Option<NotionSetupResult>> {
    if !prompt_yes_no("Enable Notion milestone report?", false)? {
        println!("[{}] Notion report disabled", style::status_ok());
        return Ok(None);
    }

    NotionClient::ensure_on_path()?;
    ensure_ntn_auth().await?;

    let client = NotionClient::new();
    println!("Searching Notion data sources…");
    let data_sources = client.search_data_sources().await?;
    if data_sources.is_empty() {
        return Err(Error::Config(
            "no Notion data sources found — create a database and share it with your account / PAT"
                .into(),
        ));
    }
    for (i, ds) in data_sources.iter().enumerate() {
        println!("  {}. {} ({})", i + 1, ds.title, ds.id);
    }
    let idx = prompt_index("Select milestones data source", data_sources.len())?;
    let data_source = &data_sources[idx];

    println!("Loading data source schema…");
    let schema = client.get_data_source(&data_source.id).await?;
    let properties = parse_data_source_properties(&schema);
    if properties.is_empty() {
        return Err(Error::Config("selected data source has no properties".into()));
    }

    let title = pick_property("title", &properties, |p| p.field_type == "title", false)?
        .ok_or_else(|| Error::Config("title property mapping is required".into()))?;

    let status_prop = pick_property(
        "status",
        &properties,
        |p| p.field_type == "status" || p.field_type == "select",
        true,
    )?;
    let status = match status_prop {
        Some(prop) => Some(build_status_mapping(&prop)?),
        None => None,
    };

    let target_date = pick_property("target_date", &properties, |p| p.field_type == "date", true)?
        .map(|p| property_ref(&p));

    let parent_relation = configure_parent_relation(&client, &properties).await?;

    let meta = NotionSyncMeta {
        data_source_id: data_source.id.clone(),
        properties: NotionPropertyMappings {
            title: property_ref(&title),
            status,
            target_date,
            parent_relation,
        },
    };

    let sync_map_path = default_sync_map_path(roadmap_dir);
    let mut sync_map = SyncMap::load(&sync_map_path)?;
    sync_map.meta.notion = Some(meta);
    sync_map.save(&sync_map_path)?;
    println!("  wrote {}", sync_map_path.display());

    Ok(Some(NotionSetupResult {
        config: NotionConfig { enabled: true, data_source_id: data_source.id.clone() },
    }))
}

async fn ensure_ntn_auth() -> Result<()> {
    if std::env::var_os("NOTION_API_TOKEN").is_some() {
        println!("[{}] using NOTION_API_TOKEN from environment", style::status_ok());
        let client = NotionClient::new();
        let who = client.auth_status().await?;
        println!("[{}] authenticated as {who}", style::status_ok());
        return Ok(());
    }

    let client = NotionClient::new();
    if let Ok(who) = client.auth_status().await {
        println!("[{}] ntn authenticated as {who}", style::status_ok());
        return Ok(());
    }
    println!("Not logged in to Notion. Starting `ntn login`…");
    run_ntn_interactive(&["login"])?;
    let who = client.auth_status().await?;
    println!("[{}] authenticated as {who}", style::status_ok());
    Ok(())
}

fn pick_property(
    local_field: &str,
    properties: &[NotionDbProperty],
    compatible: impl Fn(&NotionDbProperty) -> bool,
    allow_skip: bool,
) -> Result<Option<NotionDbProperty>> {
    let candidates: Vec<&NotionDbProperty> = properties.iter().filter(|p| compatible(p)).collect();
    if candidates.is_empty() {
        if allow_skip {
            println!(
                "[{}] no compatible Notion properties for `{local_field}` — skipping",
                style::status_ok()
            );
            return Ok(None);
        }
        return Err(Error::Config(format!("no compatible Notion properties for `{local_field}`")));
    }

    println!();
    println!("Map local `{local_field}` → Notion property:");
    if allow_skip {
        println!("  0. (skip)");
    }
    for (i, prop) in candidates.iter().enumerate() {
        println!("  {}. {} ({})", i + 1, prop.name, prop.field_type);
    }

    let default: usize = usize::from(!allow_skip); // 0 when skippable, else 1
    let answer = prompt_line(
        &format!("Select property for {local_field} [{default}]"),
        Some(&default.to_string()),
    )?;
    let n: usize =
        answer.parse().map_err(|_| Error::Config(format!("expected a number, got `{answer}`")))?;
    if allow_skip && n == 0 {
        return Ok(None);
    }
    if n == 0 || n > candidates.len() {
        let min = usize::from(!allow_skip);
        return Err(Error::Config(format!(
            "expected a number {min}..{}, got `{n}`",
            candidates.len()
        )));
    }
    Ok(Some(candidates[n - 1].clone()))
}

async fn configure_parent_relation(
    client: &NotionClient,
    properties: &[NotionDbProperty],
) -> Result<Option<NotionRelationMapping>> {
    if !prompt_yes_no("Link new milestones to an existing Notion page (relation)?", false)? {
        return Ok(None);
    }

    let relation_prop =
        pick_property("parent_relation", properties, |p| p.field_type == "relation", true)?;
    let Some(relation_prop) = relation_prop else {
        return Ok(None);
    };

    let Some(target_ds) = relation_prop.relation_data_source_id.as_deref() else {
        return Err(Error::Config(format!(
            "relation property `{}` has no target data_source_id in schema",
            relation_prop.name
        )));
    };

    println!("Loading pages from related data source…");
    let pages = client.list_data_source_pages(target_ds).await?;
    if pages.is_empty() {
        return Err(Error::Config(format!(
            "no pages found in related data source `{target_ds}` — create a target page and share that database with your Notion connection"
        )));
    }

    println!();
    println!("Select fixed page for relation `{}`:", relation_prop.name);
    println!("  0. (skip)");
    for (i, page) in pages.iter().enumerate() {
        println!("  {}. {} ({})", i + 1, page.title, page.id);
    }
    let answer = prompt_line("Select page [0]", Some("0"))?;
    let n: usize =
        answer.parse().map_err(|_| Error::Config(format!("expected a number, got `{answer}`")))?;
    if n == 0 {
        return Ok(None);
    }
    if n > pages.len() {
        return Err(Error::Config(format!("expected a number 0..{}, got `{n}`", pages.len())));
    }
    let page = &pages[n - 1];
    println!(
        "[{}] every milestone will link to `{}` via `{}`",
        style::status_ok(),
        page.title,
        relation_prop.name
    );

    Ok(Some(NotionRelationMapping {
        id: relation_prop.id.clone(),
        name: relation_prop.name.clone(),
        field_type: relation_prop.field_type.clone(),
        page_ids: vec![page.id.clone()],
    }))
}

fn build_status_mapping(prop: &NotionDbProperty) -> Result<NotionStatusMapping> {
    let mut values = match_status_values(&prop.options);
    for status in MilestoneStatus::ALL {
        let local = status.as_str();
        if values.contains_key(local) {
            continue;
        }
        if prop.options.is_empty() {
            println!(
                "[{}] warning: Notion property `{}` has no options; skipping status `{local}`",
                style::status_fail(),
                prop.name
            );
            continue;
        }
        println!();
        println!("Map local status `{local}` → Notion option on `{}`:", prop.name);
        println!("  0. (skip this status)");
        for (i, opt) in prop.options.iter().enumerate() {
            println!("  {}. {opt}", i + 1);
        }
        let answer = prompt_line("Select option [0]", Some("0"))?;
        let n: usize = answer
            .parse()
            .map_err(|_| Error::Config(format!("expected a number, got `{answer}`")))?;
        if n == 0 {
            continue;
        }
        if n > prop.options.len() {
            return Err(Error::Config(format!("expected 0..{}, got `{n}`", prop.options.len())));
        }
        values.insert(local.to_string(), prop.options[n - 1].clone());
    }

    if values.is_empty() {
        return Err(Error::Config(
            "no status values mapped; re-run setup or skip the status property".into(),
        ));
    }

    for status in MilestoneStatus::ALL {
        if !values.contains_key(status.as_str()) {
            println!(
                "[{}] warning: local status `{}` has no Notion mapping",
                style::status_fail(),
                status.as_str()
            );
        }
    }

    Ok(NotionStatusMapping {
        id: prop.id.clone(),
        name: prop.name.clone(),
        field_type: prop.field_type.clone(),
        values,
    })
}

/// Format `[platform.notion]` TOML section.
pub fn notion_toml_section(config: &NotionConfig) -> String {
    format!(
        r#"
[platform.notion]
enabled = {}
data_source_id = "{}"
"#,
        config.enabled, config.data_source_id
    )
}
