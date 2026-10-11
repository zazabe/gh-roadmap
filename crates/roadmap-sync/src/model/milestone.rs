use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::parse::LocalId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MilestoneStatus {
    Planned,
    InProgress,
    Done,
    Cancelled,
}

impl MilestoneStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }

    pub const ALL: &[MilestoneStatus] =
        &[Self::Planned, Self::InProgress, Self::Done, Self::Cancelled];
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MilestoneFrontmatter {
    pub id: String,
    pub title: String,
    pub status: MilestoneStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_date: Option<String>,
    #[serde(default)]
    pub issues: Vec<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, Option<String>>,
}

impl MilestoneFrontmatter {
    pub fn target_date(&self) -> Option<&str> {
        self.target_date.as_deref().filter(|v| !v.is_empty()).or_else(|| {
            self.fields.get("target_date").and_then(|v| v.as_deref()).filter(|v| !v.is_empty())
        })
    }
}

#[derive(Debug, Clone)]
pub struct MilestoneFile {
    pub path: PathBuf,
    pub local_id: LocalId,
    pub frontmatter: MilestoneFrontmatter,
    #[allow(dead_code)]
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct Milestone {
    pub file: MilestoneFile,
}

impl Milestone {
    pub fn id(&self) -> &str {
        &self.file.frontmatter.id
    }

    pub fn is_local_only(&self) -> bool {
        self.file.local_id.is_temporary()
    }

    pub fn github_number(&self) -> Option<u64> {
        self.file.local_id.github_number()
    }
}
