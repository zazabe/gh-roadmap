use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
pub struct RoadmapConfig {
    pub platform: PlatformConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlatformConfig {
    pub kind: String,
    pub github: Option<GithubConfig>,
    #[serde(default)]
    pub notion: Option<NotionConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubConfig {
    pub host: String,
    pub owner: String,
    pub project_number: u32,
    pub hub_repo: String,
    pub repos: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotionConfig {
    #[serde(default)]
    pub enabled: bool,
    pub database_id: String,
}

impl RoadmapConfig {
    pub fn load(roadmap_dir: &Path) -> Result<Self> {
        let path = roadmap_dir.join("roadmap.toml");
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| Error::Config(format!("failed to read {}: {e}", path.display())))?;
        toml::from_str(&raw)
            .map_err(|e| Error::Config(format!("failed to parse {}: {e}", path.display())))
    }

    pub fn github(&self) -> Result<&GithubConfig> {
        if self.platform.kind != "github" {
            return Err(Error::Config(format!(
                "unsupported platform kind `{}` (only `github` is implemented)",
                self.platform.kind
            )));
        }
        self.platform.github.as_ref().ok_or_else(|| {
            Error::Config("missing [platform.github] section in roadmap.toml".into())
        })
    }

    pub fn notion_enabled(&self) -> bool {
        self.platform.notion.as_ref().is_some_and(|n| n.enabled)
    }

    pub fn notion_config(&self) -> Result<&NotionConfig> {
        let notion = self.platform.notion.as_ref().ok_or_else(|| {
            Error::Config("missing [platform.notion] section in roadmap.toml".into())
        })?;
        if !notion.enabled {
            return Err(Error::Config("[platform.notion] is present but enabled = false".into()));
        }
        if notion.database_id.trim().is_empty() {
            return Err(Error::Config(
                "[platform.notion].database_id is required when Notion is enabled".into(),
            ));
        }
        Ok(notion)
    }

    pub fn repo_slug(&self, repo_id: &str) -> Result<String> {
        let github = self.github()?;
        github.repos.get(repo_id).cloned().ok_or_else(|| {
            Error::Config(format!(
                "unknown repo id `{repo_id}` (not listed in roadmap.toml [platform.github.repos])"
            ))
        })
    }

    pub fn full_repo(&self, repo_id: &str) -> Result<String> {
        let github = self.github()?;
        let slug = self.repo_slug(repo_id)?;
        Ok(format!("{}/{}", github.owner, slug))
    }

    pub fn hub_full_repo(&self) -> Result<String> {
        let github = self.github()?;
        Ok(format!("{}/{}", github.owner, github.hub_repo))
    }
}

pub fn default_sync_map_path(roadmap_dir: &Path) -> PathBuf {
    roadmap_dir.join(".roadmap").join("sync-map.json")
}
