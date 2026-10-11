//! Interactive Notion configuration during `roadmap setup`.

use std::path::Path;

use crate::config::{default_sync_map_path, NotionConfig};
use crate::error::{Error, Result};
use crate::model::{
    MilestoneStatus, NotionPropertyMappings, NotionStatusMapping, NotionSyncMeta, SyncMap,
};
use crate::setup::{prompt_index, prompt_line, prompt_yes_no};
use crate::style;

use super::client::{
    parse_database_properties, run_ntn_interactive, NotionClient, NotionDbProperty,
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
    println!("Searching Notion databases…");
    let databases = client.search_databases().await?;
    if databases.is_empty() {
        return Err(Error::Config(
            "no Notion databases found — create one and share it with your account / PAT".into(),
        ));
    }
    for (i, db) in databases.iter().enumerate() {
        println!("  {}. {} ({})", i + 1, db.title, db.id);
    }
    let idx = prompt_index("Select milestones database", databases.len())?;
    let database = &databases[idx];

    println!("Loading database schema…");
    let schema = client.get_database(&database.id).await?;
    let properties = parse_database_properties(&schema);
    if properties.is_empty() {
        return Err(Error::Config("selected database has no properties".into()));
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

    let meta = NotionSyncMeta {
        database_id: database.id.clone(),
        properties: NotionPropertyMappings { title: property_ref(&title), status, target_date },
    };

    let sync_map_path = default_sync_map_path(roadmap_dir);
    let mut sync_map = SyncMap::load(&sync_map_path)?;
    sync_map.meta.notion = Some(meta);
    sync_map.save(&sync_map_path)?;
    println!("  wrote {}", sync_map_path.display());

    Ok(Some(NotionSetupResult {
        config: NotionConfig { enabled: true, database_id: database.id.clone() },
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
database_id = "{}"
"#,
        config.enabled, config.database_id
    )
}
