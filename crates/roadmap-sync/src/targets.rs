//! Platform selection for `plan` / `apply` (`--platform`).

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use crate::config::RoadmapConfig;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SyncTarget {
    Github,
    Notion,
}

impl SyncTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Notion => "notion",
        }
    }
}

impl fmt::Display for SyncTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str((*self).as_str())
    }
}

impl FromStr for SyncTarget {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "github" => Ok(Self::Github),
            "notion" => Ok(Self::Notion),
            other => Err(Error::Config(format!(
                "unknown platform `{other}` (expected `github` or `notion`)"
            ))),
        }
    }
}

/// Parse `--platform github,notion`. Empty / omitted list means "use defaults".
pub fn parse_platform_list(raw: Option<&str>) -> Result<Option<BTreeSet<SyncTarget>>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Error::Config("--platform requires at least one of: github, notion".into()));
    }
    let mut set = BTreeSet::new();
    for part in trimmed.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        set.insert(SyncTarget::from_str(part)?);
    }
    if set.is_empty() {
        return Err(Error::Config("--platform requires at least one of: github, notion".into()));
    }
    Ok(Some(set))
}

/// Platforms that are configured/enabled in `roadmap.toml`.
pub fn enabled_targets(config: &RoadmapConfig) -> Result<BTreeSet<SyncTarget>> {
    let mut set = BTreeSet::new();
    // GitHub remains the primary platform while kind = github.
    let _ = config.github()?;
    set.insert(SyncTarget::Github);
    if config.notion_enabled() {
        set.insert(SyncTarget::Notion);
    }
    Ok(set)
}

/// Resolve the effective set: requested ∩ enabled, or all enabled when unset.
pub fn resolve_targets(
    config: &RoadmapConfig,
    requested: Option<&BTreeSet<SyncTarget>>,
) -> Result<BTreeSet<SyncTarget>> {
    let enabled = enabled_targets(config)?;
    let Some(requested) = requested else {
        return Ok(enabled);
    };
    let mut resolved = BTreeSet::new();
    for target in requested {
        if !enabled.contains(target) {
            return Err(Error::Config(match target {
                SyncTarget::Notion => {
                    "platform `notion` is not enabled (run `roadmap setup` or set [platform.notion] enabled = true)"
                        .into()
                }
                SyncTarget::Github => {
                    "platform `github` is not configured in roadmap.toml".into()
                }
            }));
        }
        resolved.insert(*target);
    }
    Ok(resolved)
}

pub fn format_targets(targets: &BTreeSet<SyncTarget>) -> String {
    targets.iter().copied().map(SyncTarget::as_str).collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GithubConfig, NotionConfig, PlatformConfig, RoadmapConfig};
    use std::collections::HashMap;

    fn config_with_notion(enabled: bool) -> RoadmapConfig {
        RoadmapConfig {
            platform: PlatformConfig {
                kind: "github".into(),
                github: Some(GithubConfig {
                    host: "github.com".into(),
                    owner: "acme".into(),
                    project_number: 1,
                    hub_repo: "hub".into(),
                    repos: HashMap::from([("app".into(), "app".into())]),
                }),
                notion: Some(NotionConfig { enabled, data_source_id: "ds-1".into() }),
            },
        }
    }

    #[test]
    fn parse_list() {
        let set = parse_platform_list(Some("notion, github")).unwrap().unwrap();
        assert!(set.contains(&SyncTarget::Github));
        assert!(set.contains(&SyncTarget::Notion));
        assert!(parse_platform_list(None).unwrap().is_none());
        assert!(parse_platform_list(Some("")).is_err());
        assert!(parse_platform_list(Some("jira")).is_err());
    }

    #[test]
    fn resolve_defaults_to_enabled() {
        let cfg = config_with_notion(true);
        let resolved = resolve_targets(&cfg, None).unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains(&SyncTarget::Notion));

        let cfg = config_with_notion(false);
        let resolved = resolve_targets(&cfg, None).unwrap();
        assert_eq!(resolved, BTreeSet::from([SyncTarget::Github]));
    }

    #[test]
    fn resolve_rejects_disabled_notion() {
        let cfg = config_with_notion(false);
        let requested = BTreeSet::from([SyncTarget::Notion]);
        assert!(resolve_targets(&cfg, Some(&requested)).is_err());
    }
}
