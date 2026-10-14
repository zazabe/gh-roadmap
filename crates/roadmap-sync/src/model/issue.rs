use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::parse::LocalId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueState {
    Open,
    Closed,
}

impl IssueState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IssueFrontmatter {
    pub id: String,
    pub repo: String,
    pub title: String,
    #[serde(default)]
    pub url: Option<String>,
    pub state: IssueState,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub milestone: Option<String>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone)]
pub struct IssueFile {
    pub path: PathBuf,
    pub local_id: LocalId,
    pub frontmatter: IssueFrontmatter,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub file: IssueFile,
}

impl Issue {
    pub fn id(&self) -> &str {
        &self.file.frontmatter.id
    }

    pub fn is_local_only(&self) -> bool {
        self.file.local_id.is_temporary()
    }

    pub fn github_number(&self) -> Option<u64> {
        self.file.local_id.github_number()
    }

    pub fn project_status(&self) -> &str {
        self.file
            .frontmatter
            .fields
            .get("status")
            .and_then(|value| value.as_deref())
            .filter(|value| !value.is_empty())
            .unwrap_or("Backlog")
    }
}

#[cfg(test)]
mod field_yaml_tests {
    use super::*;

    #[test]
    fn accepts_numeric_estimate() {
        let yaml = r"
id: platform-x
repo: platform
title: T
state: open
fields:
  estimate: 2.5
  priority: null
  status: Todo
";
        let fm: IssueFrontmatter = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(fm.fields.get("estimate").and_then(|v| v.as_deref()), Some("2.5"));
    }
}
