use std::collections::{HashMap, HashSet};

use crate::body_refs::{find_backticked_github_refs, find_unqualified_github_refs};
use crate::config::RoadmapConfig;
use crate::error::{Error, Result};
use crate::model::Roadmap;
use crate::parse::LocalIdKind;
use crate::style;

#[derive(Debug, Default)]
pub struct ValidationReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

pub fn validate(roadmap: &Roadmap, config: &RoadmapConfig) -> ValidationReport {
    let mut report = ValidationReport::default();
    let allowed_repos: HashSet<&str> =
        config.github().map(|gh| gh.repos.keys().map(String::as_str).collect()).unwrap_or_default();

    validate_unique_ids(roadmap, &mut report);
    validate_milestones(roadmap, &mut report);
    validate_issues(roadmap, config, &allowed_repos, &mut report);
    validate_cross_refs(roadmap, &mut report);
    validate_body_ref_style(roadmap, &mut report);
    validate_sync_map(roadmap, &mut report);
    report
}

pub fn validate_or_err(roadmap: &Roadmap, config: &RoadmapConfig) -> Result<()> {
    let report = validate(roadmap, config);
    print_report(&report);
    if report.is_ok() {
        Ok(())
    } else {
        Err(Error::Validation(report.errors.len()))
    }
}

pub fn print_report(report: &ValidationReport) {
    for warning in &report.warnings {
        eprintln!("{}: {warning}", style::status_warn());
    }
    for error in &report.errors {
        eprintln!("{}: {error}", style::status_error());
    }
    if report.is_ok() {
        println!("{} ({} warning(s))", style::ok("validation ok"), report.warnings.len());
    } else {
        eprintln!("{}: {} error(s)", style::err("validation failed"), report.errors.len());
    }
}

fn validate_unique_ids(roadmap: &Roadmap, report: &mut ValidationReport) {
    let mut seen: HashMap<&str, String> = HashMap::new();

    for milestone in &roadmap.milestones {
        if let Some(path) = seen.insert(milestone.id(), milestone.file.path.display().to_string()) {
            report.error(format!(
                "duplicate milestone id `{}` in {} and {}",
                milestone.id(),
                path,
                milestone.file.path.display()
            ));
        }
    }

    for issue in &roadmap.issues {
        if let Some(path) = seen.insert(issue.id(), issue.file.path.display().to_string()) {
            report.error(format!(
                "duplicate issue id `{}` in {} and {}",
                issue.id(),
                path,
                issue.file.path.display()
            ));
        }
    }
}

fn validate_milestones(roadmap: &Roadmap, report: &mut ValidationReport) {
    for milestone in &roadmap.milestones {
        let fm = &milestone.file.frontmatter;
        let path = milestone.file.path.display();
        if fm.id != milestone.id() {
            report.error(format!("{path}: frontmatter id `{}` != milestone id", fm.id));
        }
        if !fm.id.starts_with("milestone-") {
            report.error(format!("{path}: milestone id must start with `milestone-`"));
        }
        check_filename_id_match(
            &milestone.file.path,
            "milestone",
            &milestone_expected_slug(&fm.id),
            milestone.file.local_id.kind,
            &milestone.file.local_id.slug,
            report,
        );
    }
}

fn validate_issues(
    roadmap: &Roadmap,
    config: &RoadmapConfig,
    allowed_repos: &HashSet<&str>,
    report: &mut ValidationReport,
) {
    for issue in &roadmap.issues {
        let fm = &issue.file.frontmatter;
        let path = issue.file.path.display();
        if fm.id != issue.id() {
            report.error(format!("{path}: frontmatter id `{}` mismatch", fm.id));
        }
        if !allowed_repos.contains(fm.repo.as_str()) {
            report.error(format!("{path}: unknown repo `{}`", fm.repo));
        }
        let expected_prefix = format!("{}-", fm.repo);
        if !fm.id.starts_with(&expected_prefix) {
            report.warn(format!(
                "{path}: issue id `{}` does not start with repo prefix `{expected_prefix}`",
                fm.id
            ));
        }
        check_filename_id_match(
            &issue.file.path,
            &fm.repo,
            &issue_expected_slug(&fm.id, &fm.repo),
            issue.file.local_id.kind,
            &issue.file.local_id.slug,
            report,
        );

        if let Some(number) = issue.github_number() {
            if let Ok(full_repo) = config.full_repo(&fm.repo) {
                let expected_url = format!("https://github.com/{full_repo}/issues/{number}");
                match fm.url.as_deref() {
                    Some(url) if url == expected_url => {}
                    Some(url) => report.warn(format!(
                        "{path}: issue url `{url}` does not match expected `{expected_url}`"
                    )),
                    None => report.warn(format!(
                        "{path}: missing `url` in frontmatter (expected `{expected_url}`)"
                    )),
                }
            }
        }
    }
}

fn milestone_expected_slug(stable_id: &str) -> String {
    stable_id.strip_prefix("milestone-").unwrap_or(stable_id).to_string()
}

fn issue_expected_slug(stable_id: &str, repo: &str) -> String {
    let prefix = format!("{repo}-");
    stable_id.strip_prefix(&prefix).unwrap_or(stable_id).to_string()
}

fn check_filename_id_match(
    path: &std::path::Path,
    expected_prefix: &str,
    expected_slug: &str,
    kind: LocalIdKind,
    file_slug: &str,
    report: &mut ValidationReport,
) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let display = path.display();

    if file_slug != expected_slug {
        report.error(format!(
            "{display}: filename slug `{file_slug}` does not match expected `{expected_slug}`"
        ));
    }

    let expected_stem = crate::parse::filename_stem(expected_prefix, expected_slug, kind);
    if stem != expected_stem {
        report.error(format!(
            "{display}: filename stem `{stem}` does not match expected `{expected_stem}`"
        ));
    }
}

fn validate_cross_refs(roadmap: &Roadmap, report: &mut ValidationReport) {
    for milestone in &roadmap.milestones {
        let milestone_id = milestone.id();
        for issue_id in &milestone.file.frontmatter.issues {
            match roadmap.issue_by_id(issue_id) {
                Some(issue) => {
                    if issue.file.frontmatter.milestone.as_deref() != Some(milestone_id) {
                        report.error(format!(
                            "milestone `{milestone_id}` lists issue `{issue_id}` but issue milestone is {:?}",
                            issue.file.frontmatter.milestone
                        ));
                    }
                }
                None => report.error(format!(
                    "milestone `{milestone_id}` references unknown issue `{issue_id}`"
                )),
            }
        }
    }

    for issue in &roadmap.issues {
        if let Some(milestone_id) = &issue.file.frontmatter.milestone {
            match roadmap.milestone_by_id(milestone_id) {
                Some(milestone) => {
                    if !milestone.file.frontmatter.issues.contains(&issue.id().to_string()) {
                        report.error(format!(
                            "issue `{}` references milestone `{milestone_id}` but milestone does not list it",
                            issue.id()
                        ));
                    }
                }
                None => report.error(format!(
                    "issue `{}` references unknown milestone `{milestone_id}`",
                    issue.id()
                )),
            }
        }
    }
}

fn validate_sync_map(roadmap: &Roadmap, report: &mut ValidationReport) {
    for (id, entry) in &roadmap.sync_map.items {
        let issue = roadmap.issue_by_id(id);
        let milestone = roadmap.milestone_by_id(id);

        if issue.is_none() && milestone.is_none() {
            report.warn(format!("sync-map entry `{id}` has no matching local file"));
        }

        let local_number = issue
            .and_then(crate::model::Issue::github_number)
            .or_else(|| milestone.and_then(crate::model::Milestone::github_number));

        if let Some(number) = entry.issue_number {
            if let Some(file_number) = local_number {
                if file_number != number {
                    report.error(format!(
                        "entry `{id}` filename is #{file_number} but sync-map has #{number}"
                    ));
                }
            }
        }
    }
}

fn validate_body_ref_style(roadmap: &Roadmap, report: &mut ValidationReport) {
    for issue in &roadmap.issues {
        let refs = find_backticked_github_refs(&issue.file.body);
        if !refs.is_empty() {
            report.error(format!(
                "{}: do not wrap ticket refs in backticks: {}",
                issue.file.path.display(),
                refs.join(", ")
            ));
        }
        let refs = find_unqualified_github_refs(&issue.file.body);
        if !refs.is_empty() {
            report.error(format!(
                "{}: ticket refs must use owner/repo#id form: {}",
                issue.file.path.display(),
                refs.join(", ")
            ));
        }
    }
    for milestone in &roadmap.milestones {
        let refs = find_backticked_github_refs(&milestone.file.body);
        if !refs.is_empty() {
            report.error(format!(
                "{}: do not wrap ticket refs in backticks: {}",
                milestone.file.path.display(),
                refs.join(", ")
            ));
        }
        let refs = find_unqualified_github_refs(&milestone.file.body);
        if !refs.is_empty() {
            report.error(format!(
                "{}: ticket refs must use owner/repo#id form: {}",
                milestone.file.path.display(),
                refs.join(", ")
            ));
        }
    }
}
