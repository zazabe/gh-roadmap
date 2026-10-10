use std::path::Path;

use crate::config::RoadmapConfig;
use crate::error::Result;
use crate::platform::gh::GhPlatform;
use crate::style;

#[derive(Debug, Default)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
}

#[derive(Debug)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl DoctorReport {
    pub fn is_ok(&self) -> bool {
        self.checks.iter().all(|check| check.ok)
    }

    fn pass(name: impl Into<String>, detail: impl Into<String>) -> Check {
        Check { name: name.into(), ok: true, detail: detail.into() }
    }

    fn fail(name: impl Into<String>, detail: impl Into<String>) -> Check {
        Check { name: name.into(), ok: false, detail: detail.into() }
    }
}

pub async fn run(roadmap_dir: &Path) -> Result<(DoctorReport, RoadmapConfig)> {
    let config = RoadmapConfig::load(roadmap_dir)?;
    let github = config.github()?;
    let gh = GhPlatform::new(github.host.clone(), github.owner.clone(), github.project_number);

    let mut report = DoctorReport::default();

    match gh.auth_status().await {
        Ok(user) => {
            report.checks.push(DoctorReport::pass("gh auth", format!("logged in as {user}")));
        }
        Err(err) => {
            report.checks.push(DoctorReport::fail("gh auth", err.to_string()));
            return Ok((report, config));
        }
    }

    for (repo_id, repo_slug) in &github.repos {
        let full_repo = format!("{}/{}", github.owner, repo_slug);
        match gh.repo_access(&full_repo).await {
            Ok(detail) => {
                report.checks.push(DoctorReport::pass(format!("repo read ({repo_id})"), detail));
            }
            Err(err) => report
                .checks
                .push(DoctorReport::fail(format!("repo read ({repo_id})"), err.to_string())),
        }
    }

    let probe_repo = config.hub_full_repo()?;
    match gh.probe_issue_write(&probe_repo).await {
        Ok(()) => report
            .checks
            .push(DoctorReport::pass("issue write", format!("can create issues in {probe_repo}"))),
        Err(err) => report.checks.push(DoctorReport::fail("issue write", err.to_string())),
    }

    match gh
        .probe_project_write(&crate::platform::ProjectRef {
            owner: github.owner.clone(),
            number: github.project_number,
        })
        .await
    {
        Ok(()) => report.checks.push(DoctorReport::pass(
            "project write",
            format!("can update project #{}", github.project_number),
        )),
        Err(err) => report.checks.push(DoctorReport::fail("project write", err.to_string())),
    }

    Ok((report, config))
}

pub fn print_report(report: &DoctorReport, config: &RoadmapConfig) {
    for check in &report.checks {
        let status = if check.ok { style::status_ok() } else { style::status_fail() };
        println!("[{status}] {} — {}", check.name, check.detail);
    }
    if report.is_ok() {
        println!("{}", style::ok("doctor: all checks passed"));
    } else {
        eprintln!("{}", style::err("doctor: fix failing checks before `roadmap apply --yes`"));
        eprintln!();
        eprintln!("Fine-grained PAT requirements:");
        if let Ok(github) = config.github() {
            let repos: Vec<_> = github
                .repos
                .values()
                .map(|slug| format!("{}/{}", github.owner, slug))
                .collect();
            eprintln!("  - Repository access: {}", repos.join(", "));
            eprintln!("  - Repository permissions > Issues: Read and write");
            eprintln!(
                "  - Organization/user permissions > Projects: Read and write (board #{})",
                github.project_number
            );
        } else {
            eprintln!("  - Repository access: all repos listed in roadmap/roadmap.toml");
            eprintln!("  - Repository permissions > Issues: Read and write");
            eprintln!("  - Organization/user permissions > Projects: Read and write");
        }
        eprintln!();
        eprintln!("Or re-auth with classic scopes:");
        eprintln!("  gh auth refresh -h github.com -s repo,project");
    }
}
