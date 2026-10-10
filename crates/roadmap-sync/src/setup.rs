//! Interactive `roadmap setup`: auth, discover project/repos, scaffold tree.

use std::collections::BTreeMap;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Deserialize;
use tokio::process::Command;

use crate::doctor;
use crate::error::{Error, PlatformError, Result};
use crate::style;

const DEFAULT_HOST: &str = "github.com";
const REQUIRED_SCOPES: &[&str] = &["repo", "project"];
const DEFAULT_SKILL_DIR: &str = ".cursor/skills/roadmap";

const SKILL_FILES: &[(&str, &str)] = &[
    ("SKILL.md", include_str!("../../../.cursor/skills/roadmap/SKILL.md")),
    ("examples.md", include_str!("../../../.cursor/skills/roadmap/examples.md")),
    ("projects.md", include_str!("../../../.cursor/skills/roadmap/projects.md")),
    ("templates.md", include_str!("../../../.cursor/skills/roadmap/templates.md")),
];

pub struct SetupOptions {
    pub force: bool,
    pub no_skill: bool,
    pub skill_dir: Option<PathBuf>,
}

pub async fn run(roadmap_dir: &Path, opts: SetupOptions) -> Result<()> {
    if !io::stdin().is_terminal() {
        return Err(Error::Config(
            "roadmap setup requires an interactive terminal (stdin is not a TTY)".into(),
        ));
    }

    println!("{}", style::heading("roadmap setup"));
    println!();

    ensure_gh_on_path()?;
    let host = DEFAULT_HOST.to_string();
    ensure_auth(&host).await?;

    let default_owner = default_owner().await;
    let owner = prompt_line(
        &format!("GitHub owner (org or user) [{}]", default_owner.as_deref().unwrap_or("")),
        default_owner.as_deref(),
    )?;
    if owner.is_empty() {
        return Err(Error::Config("owner is required".into()));
    }

    let project = pick_project(&owner).await?;
    let repos = pick_repos(&owner).await?;
    let hub_repo = pick_hub_repo(&repos)?;

    scaffold_tree(roadmap_dir, &owner, &repos, opts.force)?;
    write_roadmap_toml(roadmap_dir, &host, &owner, project.number, &hub_repo, &repos, opts.force)?;

    maybe_install_skill(&opts)?;

    println!();
    println!("{}", style::ok("setup complete — running doctor…"));
    println!();
    let (report, config) = doctor::run(roadmap_dir).await?;
    doctor::print_report(&report, &config);
    if report.is_ok() {
        println!();
        println!(
            "{}",
            style::ok(
                "next: edit projects/, add milestones/issues, then `roadmap plan` / `roadmap apply --yes`"
            )
        );
        Ok(())
    } else {
        Err(Error::Auth)
    }
}

fn ensure_gh_on_path() -> Result<()> {
    match std::process::Command::new("gh").arg("--version").output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(_) | Err(_) => {
            Err(Error::Config("`gh` not found on PATH — install https://cli.github.com/".into()))
        }
    }
}

async fn ensure_auth(host: &str) -> Result<()> {
    if std::env::var_os("GH_TOKEN").is_some() || std::env::var_os("GITHUB_TOKEN").is_some() {
        println!("[{}] using GH_TOKEN / GITHUB_TOKEN from environment", style::status_ok());
        let login = gh_json::<User>(&["api", "user"]).await?.login;
        println!("[{}] authenticated as {login}", style::status_ok());
        return Ok(());
    }

    let status = gh_json::<AuthStatus>(&["auth", "status", "--json", "hosts"]).await?;
    let accounts = status.hosts.get(host).cloned().unwrap_or_default();
    let active = accounts.into_iter().find(|a| a.active.unwrap_or(true));

    match active {
        None => {
            println!("Not logged in to {host}. Starting `gh auth login`…");
            run_gh_interactive(&[
                "auth",
                "login",
                "-h",
                host,
                "-s",
                "repo,project",
                "--git-protocol",
                "https",
            ])?;
        }
        Some(account) => {
            let scopes = parse_scopes(account.scopes.as_deref().unwrap_or(""));
            let missing: Vec<&str> = REQUIRED_SCOPES
                .iter()
                .copied()
                .filter(|s| !scopes.iter().any(|have| have == s))
                .collect();
            if missing.is_empty() {
                println!(
                    "[{}] logged in to {host} as {}",
                    style::status_ok(),
                    account.login.unwrap_or_else(|| "(unknown)".into())
                );
                return Ok(());
            }
            println!(
                "Logged in as {}, but missing scopes: {}. Running `gh auth refresh`…",
                account.login.as_deref().unwrap_or("unknown"),
                missing.join(", ")
            );
            run_gh_interactive(&["auth", "refresh", "-h", host, "-s", "repo,project"])?;
        }
    }

    let login = gh_json::<User>(&["api", "user"]).await?.login;
    println!("[{}] authenticated as {login}", style::status_ok());
    Ok(())
}

fn parse_scopes(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

async fn default_owner() -> Option<String> {
    if let Some(owner) = owner_from_git_remote() {
        return Some(owner);
    }
    gh_json::<User>(&["api", "user"]).await.ok().map(|u| u.login)
}

fn owner_from_git_remote() -> Option<String> {
    let output =
        std::process::Command::new("git").args(["remote", "get-url", "origin"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_owner_from_remote(&url)
}

fn parse_owner_from_remote(url: &str) -> Option<String> {
    // git@github.com:OWNER/REPO.git or https://github.com/OWNER/REPO.git
    let path = if let Some(rest) = url.strip_prefix("git@") {
        rest.split_once(':')?.1
    } else if let Some(idx) = url.find("://") {
        let after = &url[idx + 3..];
        after.split_once('/')?.1
    } else {
        return None;
    };
    let owner = path.trim_matches('/').split('/').next()?;
    if owner.is_empty() {
        None
    } else {
        Some(owner.to_string())
    }
}

async fn pick_project(owner: &str) -> Result<ProjectEntry> {
    println!();
    println!("{}", style::heading("GitHub Projects"));
    let list = gh_json::<ProjectList>(&[
        "project", "list", "--owner", owner, "--limit", "50", "--format", "json",
    ])
    .await?;
    if list.projects.is_empty() {
        return Err(Error::Config(format!(
            "no GitHub Projects found for `{owner}` — create one, then re-run setup"
        )));
    }
    for (i, p) in list.projects.iter().enumerate() {
        println!("  {}) #{}  {}", i + 1, p.number, p.title);
    }
    let idx = prompt_index("Select project", list.projects.len())?;
    Ok(list.projects[idx].clone())
}

async fn pick_repos(owner: &str) -> Result<Vec<(String, String)>> {
    println!();
    println!("{}", style::heading("Repositories"));
    let repos = gh_json::<Vec<RepoEntry>>(&[
        "repo",
        "list",
        owner,
        "--limit",
        "100",
        "--json",
        "name",
        "--no-archived",
        "--source",
    ])
    .await?;
    if repos.is_empty() {
        return Err(Error::Config(format!("no repositories found for `{owner}`")));
    }
    for (i, r) in repos.iter().enumerate() {
        println!("  {}) {}", i + 1, r.name);
    }
    println!("Enter numbers separated by commas, or `all`.");
    let selected = prompt_multi_index("Select repos for the roadmap", repos.len())?;
    let mut mapped = Vec::new();
    for idx in selected {
        let name = repos[idx].name.clone();
        let key_default = name.clone();
        let key = prompt_line(
            &format!("  Frontmatter repo key for `{name}` [{key_default}]"),
            Some(&key_default),
        )?;
        let key = if key.is_empty() { key_default } else { key };
        mapped.push((key, name));
    }
    if mapped.is_empty() {
        return Err(Error::Config("select at least one repository".into()));
    }
    Ok(mapped)
}

fn pick_hub_repo(repos: &[(String, String)]) -> Result<String> {
    println!();
    println!("{}", style::heading("Hub repo"));
    println!("Epics (milestones) are created as issues in the hub repo.");
    for (i, (_key, name)) in repos.iter().enumerate() {
        println!("  {}) {}", i + 1, name);
    }
    let default = 1;
    let idx = prompt_index_default("Select hub repo", repos.len(), default)?;
    Ok(repos[idx].1.clone())
}

fn scaffold_tree(
    roadmap_dir: &Path,
    owner: &str,
    repos: &[(String, String)],
    force: bool,
) -> Result<()> {
    println!();
    println!("{}", style::heading("Scaffold"));

    std::fs::create_dir_all(roadmap_dir.join("milestones"))?;
    std::fs::create_dir_all(roadmap_dir.join("issues"))?;
    std::fs::create_dir_all(roadmap_dir.join(".roadmap"))?;

    let sync_map = roadmap_dir.join(".roadmap").join("sync-map.json");
    if !sync_map.exists() || force {
        std::fs::write(&sync_map, "{}\n")?;
        println!("  wrote {}", sync_map.display());
    }

    let projects_dir = projects_dir_for(roadmap_dir);
    std::fs::create_dir_all(&projects_dir)?;

    let overview = projects_dir.join("README.md");
    if !overview.exists() || force {
        let mut rows = String::new();
        for (key, name) in repos {
            use std::fmt::Write as _;
            let _ = writeln!(rows, "| [{key}](./{key}/) | `{owner}/{name}` | Current-state docs |");
        }
        let body = format!(
            r"# Projects

Current-state reference for product repositories. Keep these updated when
roadmap work ships (see `.cursor/skills/roadmap/projects.md`).

| Project | Repo | Role |
|---------|------|------|
{rows}
## Architecture

```
Clients
   ↓
{owner}/…
```
"
        );
        std::fs::write(&overview, body)?;
        println!("  wrote {}", overview.display());
    }

    for (key, name) in repos {
        let dir = projects_dir.join(key);
        std::fs::create_dir_all(&dir)?;
        let readme = dir.join("README.md");
        if !readme.exists() || force {
            let body = format!(
                r"# {title}

**Repo:** `{owner}/{name}`

Current-state notes for this repository. Describe what exists today — not planned work.

## Overview

- _TODO: purpose of this repo_

## Layout

- _TODO: key packages / services_

## Deploy

- _TODO: how this ships_
",
                title = title_case(key),
            );
            std::fs::write(&readme, body)?;
            println!("  wrote {}", readme.display());
        }
    }

    Ok(())
}

fn projects_dir_for(roadmap_dir: &Path) -> PathBuf {
    match roadmap_dir.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("projects"),
        _ => PathBuf::from("projects"),
    }
}

fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn write_roadmap_toml(
    roadmap_dir: &Path,
    host: &str,
    owner: &str,
    project_number: u32,
    hub_repo: &str,
    repos: &[(String, String)],
    force: bool,
) -> Result<()> {
    let path = roadmap_dir.join("roadmap.toml");
    if path.exists() && !force {
        return Err(Error::Config(format!(
            "{} already exists (pass --force to overwrite)",
            path.display()
        )));
    }
    std::fs::create_dir_all(roadmap_dir)?;

    let mut repo_lines = BTreeMap::new();
    for (key, name) in repos {
        repo_lines.insert(key.clone(), name.clone());
    }
    let mut repos_toml = String::new();
    for (k, v) in &repo_lines {
        use std::fmt::Write as _;
        let _ = writeln!(repos_toml, "{k} = \"{v}\"");
    }

    let body = format!(
        r#"[platform]
kind = "github"

[platform.github]
host = "{host}"
owner = "{owner}"
project_number = {project_number}
hub_repo = "{hub_repo}"

[platform.github.repos]
{repos_toml}"#
    );
    std::fs::write(&path, body)?;
    println!("  wrote {}", path.display());
    Ok(())
}

fn maybe_install_skill(opts: &SetupOptions) -> Result<()> {
    if opts.no_skill {
        println!("[{}] skipped skill install (--no-skill)", style::status_ok());
        return Ok(());
    }

    let dest = opts.skill_dir.clone().unwrap_or_else(|| PathBuf::from(DEFAULT_SKILL_DIR));
    let install = if opts.skill_dir.is_some() {
        true
    } else {
        prompt_yes_no(&format!("Install agent skill to {}?", dest.display()), true)?
    };
    if !install {
        println!("[{}] skipped skill install", style::status_ok());
        return Ok(());
    }

    if dest.exists() && !opts.force {
        let entries = std::fs::read_dir(&dest)?.count();
        if entries > 0 {
            return Err(Error::Config(format!(
                "{} is not empty (pass --force to overwrite)",
                dest.display()
            )));
        }
    }
    std::fs::create_dir_all(&dest)?;
    for (name, contents) in SKILL_FILES {
        let path = dest.join(name);
        std::fs::write(&path, contents)?;
        println!("  wrote {}", path.display());
    }
    println!("[{}] skill installed", style::status_ok());
    Ok(())
}

fn prompt_line(label: &str, default: Option<&str>) -> Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let trimmed = line.trim().to_string();
    if trimmed.is_empty() {
        Ok(default.unwrap_or("").to_string())
    } else {
        Ok(trimmed)
    }
}

fn prompt_yes_no(label: &str, default_yes: bool) -> Result<bool> {
    let hint = if default_yes { "Y/n" } else { "y/N" };
    let answer = prompt_line(&format!("{label} [{hint}]"), None)?;
    if answer.is_empty() {
        return Ok(default_yes);
    }
    match answer.to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => Err(Error::Config(format!("expected y or n, got `{answer}`"))),
    }
}

fn prompt_index(label: &str, len: usize) -> Result<usize> {
    prompt_index_default(label, len, 1)
}

fn prompt_index_default(label: &str, len: usize, default: usize) -> Result<usize> {
    let answer = prompt_line(&format!("{label} [{default}]"), Some(&default.to_string()))?;
    let n: usize = answer
        .parse()
        .map_err(|_| Error::Config(format!("expected a number 1..{len}, got `{answer}`")))?;
    if n == 0 || n > len {
        return Err(Error::Config(format!("expected a number 1..{len}, got `{n}`")));
    }
    Ok(n - 1)
}

fn prompt_multi_index(label: &str, len: usize) -> Result<Vec<usize>> {
    let answer = prompt_line(label, None)?;
    if answer.eq_ignore_ascii_case("all") {
        return Ok((0..len).collect());
    }
    let mut out = Vec::new();
    for part in answer.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let n: usize = part
            .parse()
            .map_err(|_| Error::Config(format!("expected numbers or `all`, got `{answer}`")))?;
        if n == 0 || n > len {
            return Err(Error::Config(format!("expected a number 1..{len}, got `{n}`")));
        }
        let idx = n - 1;
        if !out.contains(&idx) {
            out.push(idx);
        }
    }
    Ok(out)
}

fn run_gh_interactive(args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("gh")
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| Error::Config(format!("failed to spawn gh: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Config(format!("gh {} failed", args.join(" "))))
    }
}

async fn gh_json<T: serde::de::DeserializeOwned>(args: &[&str]) -> Result<T> {
    let output = Command::new("gh")
        .args(args)
        .output()
        .await
        .map_err(|e| Error::Config(format!("failed to spawn gh: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        let msg = if stderr.is_empty() { stdout } else { stderr };
        return Err(PlatformError::CommandFailed { program: "gh".into(), message: msg }.into());
    }
    serde_json::from_str(&stdout)
        .map_err(|source| PlatformError::InvalidJson { program: "gh".into(), source }.into())
}

#[derive(Debug, Deserialize)]
struct User {
    login: String,
}

#[derive(Debug, Deserialize)]
struct AuthStatus {
    hosts: BTreeMap<String, Vec<AuthAccount>>,
}

#[derive(Debug, Clone, Deserialize)]
struct AuthAccount {
    login: Option<String>,
    scopes: Option<String>,
    active: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ProjectList {
    projects: Vec<ProjectEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProjectEntry {
    number: u32,
    title: String,
}

#[derive(Debug, Deserialize)]
struct RepoEntry {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::parse_owner_from_remote;

    #[test]
    fn parses_ssh_remote() {
        assert_eq!(parse_owner_from_remote("git@github.com:acme/app.git").as_deref(), Some("acme"));
    }

    #[test]
    fn parses_https_remote() {
        assert_eq!(
            parse_owner_from_remote("https://github.com/acme/app.git").as_deref(),
            Some("acme")
        );
    }
}
