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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueSyncEntry {
    pub repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_item_id: Option<String>,
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
