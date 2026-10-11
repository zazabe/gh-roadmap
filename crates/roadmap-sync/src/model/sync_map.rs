use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Issue, Milestone};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncMap {
    #[serde(rename = "_meta", default)]
    pub meta: SyncMapMeta,
    #[serde(default)]
    pub items: HashMap<String, IssueSyncEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncMapMeta {
    #[serde(default)]
    pub fields: SyncFieldMappings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notion: Option<NotionSyncMeta>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncFieldMappings {
    #[serde(default)]
    pub issue: HashMap<String, ProjectFieldMapping>,
    #[serde(default)]
    pub milestone: HashMap<String, ProjectFieldMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFieldMapping {
    pub field_id: String,
    pub field_name: String,
    #[serde(rename = "type")]
    pub field_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotionSyncMeta {
    pub database_id: String,
    pub properties: NotionPropertyMappings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotionPropertyMappings {
    pub title: NotionPropertyRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<NotionStatusMapping>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_date: Option<NotionPropertyRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotionPropertyRef {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotionStatusMapping {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    /// Local milestone status (`planned`, …) → Notion option name.
    pub values: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueSyncEntry {
    pub repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notion_page_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl SyncMap {
    pub fn load(path: &Path) -> crate::error::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        if raw.trim().is_empty() || raw.trim() == "{}" {
            return Ok(Self::default());
        }
        serde_json::from_str(&raw).map_err(|e| crate::error::Error::Parse {
            path: path.display().to_string(),
            message: format!("invalid sync-map JSON: {e}"),
        })
    }

    pub fn save(&self, path: &Path) -> crate::error::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(path, format!("{raw}\n"))?;
        Ok(())
    }

    pub fn issue_entry(&self, id: &str) -> Option<&IssueSyncEntry> {
        self.items.get(id)
    }

    pub fn epic_url(&self, milestone_id: &str, hub_repo: &str) -> Option<String> {
        self.issue_entry(milestone_id).and_then(|entry| {
            entry
                .issue_number
                .map(|number| format!("https://github.com/{hub_repo}/issues/{number}"))
        })
    }

    pub fn upsert_issue(&mut self, id: impl Into<String>, entry: IssueSyncEntry) {
        self.items.insert(id.into(), entry);
    }

    /// Update GitHub fields while preserving an existing Notion page id.
    pub fn upsert_github_entry(
        &mut self,
        id: &str,
        repo: String,
        issue_number: Option<u64>,
        project_item_id: Option<String>,
        updated_at: Option<String>,
    ) {
        let notion_page_id = self.items.get(id).and_then(|e| e.notion_page_id.clone());
        self.upsert_issue(
            id,
            IssueSyncEntry { repo, issue_number, project_item_id, notion_page_id, updated_at },
        );
    }

    pub fn set_notion_page_id(&mut self, id: &str, page_id: String, updated_at: Option<String>) {
        let entry = self.items.entry(id.to_string()).or_insert_with(|| IssueSyncEntry {
            repo: String::new(),
            issue_number: None,
            project_item_id: None,
            notion_page_id: None,
            updated_at: None,
        });
        entry.notion_page_id = Some(page_id);
        if updated_at.is_some() {
            entry.updated_at = updated_at;
        }
    }
}

#[derive(Debug, Clone)]
pub struct Roadmap {
    pub milestones: Vec<Milestone>,
    pub issues: Vec<Issue>,
    pub sync_map: SyncMap,
}

impl Roadmap {
    pub fn issue_by_id(&self, id: &str) -> Option<&Issue> {
        self.issues.iter().find(|issue| issue.id() == id)
    }

    pub fn milestone_by_id(&self, id: &str) -> Option<&Milestone> {
        self.milestones.iter().find(|milestone| milestone.id() == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_notion_meta_and_page_id() {
        let raw = r#"{
  "_meta": {
    "notion": {
      "database_id": "db-1",
      "properties": {
        "title": { "id": "title", "name": "Name", "type": "title" },
        "status": {
          "id": "abc",
          "name": "Status",
          "type": "status",
          "values": { "planned": "Planned" }
        }
      }
    }
  },
  "items": {
    "milestone-x": {
      "repo": "acme/hub",
      "issue_number": 1,
      "notion_page_id": "page-1"
    }
  }
}"#;
        let map: SyncMap = serde_json::from_str(raw).unwrap();
        assert_eq!(map.meta.notion.as_ref().unwrap().database_id, "db-1");
        assert_eq!(map.items["milestone-x"].notion_page_id.as_deref(), Some("page-1"));
        map.meta.notion.as_ref().unwrap().properties.status.as_ref().unwrap();
    }

    #[test]
    fn upsert_github_preserves_notion_page() {
        let mut map = SyncMap::default();
        map.set_notion_page_id("m1", "page-9".into(), None);
        map.upsert_github_entry("m1", "acme/hub".into(), Some(3), Some("item".into()), None);
        assert_eq!(map.items["m1"].notion_page_id.as_deref(), Some("page-9"));
        assert_eq!(map.items["m1"].issue_number, Some(3));
    }
}
