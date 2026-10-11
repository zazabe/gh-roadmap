use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::model::{Milestone, MilestoneStatus, NotionPropertyMappings, NotionPropertyRef};

/// Build Notion page `properties` object from a milestone + mapping.
pub fn build_page_properties(
    milestone: &Milestone,
    mappings: &NotionPropertyMappings,
) -> Result<Value> {
    let mut properties = serde_json::Map::new();

    properties
        .insert(mappings.title.name.clone(), title_property(&milestone.file.frontmatter.title));

    if let Some(status_map) = &mappings.status {
        let local = milestone.file.frontmatter.status.as_str();
        let notion_value = status_map.values.get(local).ok_or_else(|| {
            Error::Config(format!(
                "no Notion status mapping for local status `{local}` (milestone `{}`)",
                milestone.id()
            ))
        })?;
        properties.insert(
            status_map.name.clone(),
            status_or_select_property(&status_map.field_type, notion_value)?,
        );
    }

    if let Some(date_prop) = &mappings.target_date {
        if let Some(date) = milestone.file.frontmatter.target_date() {
            if let Some(notion_date) = normalize_notion_date(date) {
                properties.insert(date_prop.name.clone(), date_property(&notion_date));
            }
        }
    }

    Ok(Value::Object(properties))
}

fn title_property(title: &str) -> Value {
    json!({
        "title": [{ "text": { "content": title } }]
    })
}

fn status_or_select_property(field_type: &str, name: &str) -> Result<Value> {
    match field_type {
        "status" => Ok(json!({ "status": { "name": name } })),
        "select" => Ok(json!({ "select": { "name": name } })),
        other => Err(Error::Config(format!(
            "unsupported Notion status property type `{other}` (expected status or select)"
        ))),
    }
}

fn date_property(start: &str) -> Value {
    json!({ "date": { "start": start } })
}

/// Notion date properties need ISO dates. Quarters like `2026-Q3` are skipped.
pub fn normalize_notion_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let bytes = raw.as_bytes();
    // YYYY-MM-DD
    if raw.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..].iter().all(u8::is_ascii_digit)
    {
        return Some(raw.to_string());
    }
    // YYYY-MM
    if raw.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..].iter().all(u8::is_ascii_digit)
    {
        return Some(format!("{raw}-01"));
    }
    None
}

pub fn match_status_values(options: &[String]) -> std::collections::HashMap<String, String> {
    let mut values = std::collections::HashMap::new();
    for status in MilestoneStatus::ALL {
        let local = status.as_str();
        if let Some(opt) = find_option(options, local)
            .or_else(|| find_option(options, &local.replace('_', " ")))
            .or_else(|| find_option(options, humanize(local).as_str()))
        {
            values.insert(local.to_string(), opt);
        }
    }
    values
}

fn humanize(local: &str) -> String {
    local
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => format!("{}{}", c.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_option(options: &[String], needle: &str) -> Option<String> {
    options.iter().find(|opt| opt.eq_ignore_ascii_case(needle)).cloned()
}

pub fn property_ref(prop: &crate::notion::client::NotionDbProperty) -> NotionPropertyRef {
    NotionPropertyRef {
        id: prop.id.clone(),
        name: prop.name.clone(),
        field_type: prop.field_type.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        MilestoneFile, MilestoneFrontmatter, MilestoneStatus, NotionPropertyMappings,
        NotionPropertyRef, NotionStatusMapping,
    };
    use crate::parse::{LocalId, LocalIdKind};
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;

    fn milestone(status: MilestoneStatus, target_date: Option<&str>) -> Milestone {
        Milestone {
            file: MilestoneFile {
                path: PathBuf::from("milestone-T1-x.md"),
                local_id: LocalId {
                    entity_prefix: "milestone".into(),
                    slug: "x".into(),
                    kind: LocalIdKind::Temporary(1),
                },
                frontmatter: MilestoneFrontmatter {
                    id: "milestone-x".into(),
                    title: "Epic 1: X".into(),
                    status,
                    target_date: target_date.map(str::to_string),
                    issues: vec![],
                    fields: BTreeMap::new(),
                },
                body: String::new(),
            },
        }
    }

    #[test]
    fn builds_title_and_status() {
        let mappings = NotionPropertyMappings {
            title: NotionPropertyRef {
                id: "title".into(),
                name: "Name".into(),
                field_type: "title".into(),
            },
            status: Some(NotionStatusMapping {
                id: "s".into(),
                name: "Status".into(),
                field_type: "status".into(),
                values: HashMap::from([("planned".into(), "Planned".into())]),
            }),
            target_date: None,
        };
        let props =
            build_page_properties(&milestone(MilestoneStatus::Planned, None), &mappings).unwrap();
        assert_eq!(
            props.pointer("/Name/title/0/text/content").and_then(Value::as_str),
            Some("Epic 1: X")
        );
        assert_eq!(props.pointer("/Status/status/name").and_then(Value::as_str), Some("Planned"));
    }

    #[test]
    fn skips_quarter_dates() {
        assert!(normalize_notion_date("2026-Q3").is_none());
        assert_eq!(normalize_notion_date("2026-07-15").as_deref(), Some("2026-07-15"));
        assert_eq!(normalize_notion_date("2026-07").as_deref(), Some("2026-07-01"));
    }

    #[test]
    fn matches_status_options() {
        let values = match_status_values(&[
            "Planned".into(),
            "In Progress".into(),
            "Done".into(),
            "Cancelled".into(),
        ]);
        assert_eq!(values.get("planned").map(String::as_str), Some("Planned"));
        assert_eq!(values.get("in_progress").map(String::as_str), Some("In Progress"));
    }
}
