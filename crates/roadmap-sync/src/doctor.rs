use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::config::RoadmapConfig;
use crate::error::Result;
use crate::loader::load;
use crate::model::Roadmap;
use crate::platform::gh::GhPlatform;
use crate::platform::RemoteIssue;
use crate::style;

const PROBE_TITLE_PREFIX: &str = "__roadmap_doctor_probe__";
const OPEN_ISSUE_LIST_LIMIT: u32 = 1000;

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

#[derive(Debug, Clone, Default)]
pub struct DoctorOptions {
    pub dedup: bool,
    pub yes: bool,
}

/// Local roadmap ticket expected to exist once on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTicket {
    pub id: String,
    pub title: String,
    pub full_repo: String,
    pub preferred_number: Option<u64>,
}

/// One local title with multiple open GitHub issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub local_id: String,
    pub title: String,
    pub full_repo: String,
    pub keeper: RemoteIssueBrief,
    pub extras: Vec<RemoteIssueBrief>,
    /// Preferred number from sync-map/filename was not among open title matches.
    pub preferred_missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteIssueBrief {
    pub number: u64,
    pub title: String,
    pub url: String,
}

impl From<&RemoteIssue> for RemoteIssueBrief {
    fn from(issue: &RemoteIssue) -> Self {
        Self { number: issue.number, title: issue.title.clone(), url: issue.url.clone() }
    }
}

#[derive(Debug, Default)]
pub struct DedupPlan {
    pub groups: Vec<DuplicateGroup>,
    pub warnings: Vec<String>,
}

pub async fn run(
    roadmap_dir: &Path,
    options: DoctorOptions,
) -> Result<(DoctorReport, RoadmapConfig, Option<DedupPlan>)> {
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
            return Ok((report, config, None));
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

    if config.notion_enabled() {
        check_notion(&mut report, &config).await;
    }

    let dedup_plan = if options.dedup {
        if report.is_ok() {
            Some(run_dedup(roadmap_dir, &config, &gh, options.yes, &mut report).await?)
        } else {
            report.checks.push(DoctorReport::fail(
                "dedup",
                "skipped — fix failing checks before running duplicate cleanup",
            ));
            None
        }
    } else {
        None
    };

    Ok((report, config, dedup_plan))
}

async fn run_dedup(
    roadmap_dir: &Path,
    config: &RoadmapConfig,
    gh: &GhPlatform,
    apply: bool,
    report: &mut DoctorReport,
) -> Result<DedupPlan> {
    let roadmap = load(roadmap_dir)?;
    let locals = collect_local_tickets(&roadmap, config)?;
    let mut repos: Vec<String> = locals.iter().map(|t| t.full_repo.clone()).collect();
    repos.sort();
    repos.dedup();

    let mut remotes_by_repo: HashMap<String, Vec<RemoteIssueBrief>> = HashMap::new();
    for full_repo in &repos {
        let issues = gh.list_open_issues(full_repo, OPEN_ISSUE_LIST_LIMIT).await?;
        remotes_by_repo.insert(
            full_repo.clone(),
            issues
                .iter()
                .filter(|issue| !issue.title.starts_with(PROBE_TITLE_PREFIX))
                .map(RemoteIssueBrief::from)
                .collect(),
        );
        report.checks.push(DoctorReport::pass(
            format!("dedup scan ({full_repo})"),
            format!("{} open issue(s)", remotes_by_repo[full_repo].len()),
        ));
    }

    let plan = find_duplicate_groups(&locals, &remotes_by_repo);

    if plan.groups.is_empty() {
        report.checks.push(DoctorReport::pass(
            "dedup",
            "no open title duplicates for local roadmap tickets",
        ));
        return Ok(plan);
    }

    let extra_count: usize = plan.groups.iter().map(|g| g.extras.len()).sum();
    if apply {
        for group in &plan.groups {
            for extra in &group.extras {
                let comment = format!(
                    "Closed by `roadmap doctor --dedup --yes`: duplicate of #{} (local id `{}`).",
                    group.keeper.number, group.local_id
                );
                gh.close_issue_with_comment(&group.full_repo, extra.number, &comment).await?;
            }
        }
        report.checks.push(DoctorReport::pass(
            "dedup",
            format!(
                "closed {extra_count} duplicate issue(s) across {} title group(s)",
                plan.groups.len()
            ),
        ));
    } else {
        report.checks.push(DoctorReport::fail(
            "dedup",
            format!(
                "found {extra_count} duplicate open issue(s) across {} title group(s); re-run with --dedup --yes to close extras",
                plan.groups.len()
            ),
        ));
    }

    Ok(plan)
}

pub fn collect_local_tickets(
    roadmap: &Roadmap,
    config: &RoadmapConfig,
) -> Result<Vec<LocalTicket>> {
    let hub = config.hub_full_repo()?;
    let mut tickets = Vec::new();

    for milestone in &roadmap.milestones {
        let preferred =
            roadmap.sync_map.resolved_issue_number(milestone.id(), milestone.github_number());
        tickets.push(LocalTicket {
            id: milestone.id().to_string(),
            title: milestone.file.frontmatter.title.clone(),
            full_repo: hub.clone(),
            preferred_number: preferred,
        });
    }

    for issue in &roadmap.issues {
        let full_repo = config.full_repo(&issue.file.frontmatter.repo)?;
        let preferred = roadmap.sync_map.resolved_issue_number(issue.id(), issue.github_number());
        tickets.push(LocalTicket {
            id: issue.id().to_string(),
            title: issue.file.frontmatter.title.clone(),
            full_repo,
            preferred_number: preferred,
        });
    }

    Ok(tickets)
}

/// Match open remote issues to local tickets by exact title within each repo.
pub fn find_duplicate_groups(
    locals: &[LocalTicket],
    remotes_by_repo: &HashMap<String, Vec<RemoteIssueBrief>>,
) -> DedupPlan {
    let mut plan = DedupPlan::default();

    let mut title_owners: BTreeMap<(String, String), Vec<&LocalTicket>> = BTreeMap::new();
    for ticket in locals {
        title_owners
            .entry((ticket.full_repo.clone(), ticket.title.clone()))
            .or_default()
            .push(ticket);
    }

    let mut ambiguous: HashSet<(String, String)> = HashSet::new();
    for ((repo, title), owners) in &title_owners {
        if owners.len() > 1 {
            ambiguous.insert((repo.clone(), title.clone()));
            let ids: Vec<_> = owners.iter().map(|t| t.id.as_str()).collect();
            plan.warnings.push(format!(
                "skip `{title}` in {repo}: multiple local tickets share this title ({})",
                ids.join(", ")
            ));
        }
    }

    for ticket in locals {
        if ambiguous.contains(&(ticket.full_repo.clone(), ticket.title.clone())) {
            continue;
        }
        let Some(remotes) = remotes_by_repo.get(&ticket.full_repo) else {
            continue;
        };
        let mut matches: Vec<RemoteIssueBrief> =
            remotes.iter().filter(|r| r.title == ticket.title).cloned().collect();
        if matches.len() <= 1 {
            continue;
        }
        matches.sort_by_key(|r| r.number);

        let preferred_missing =
            ticket.preferred_number.is_some_and(|n| matches.iter().all(|m| m.number != n));

        let keeper_number = ticket
            .preferred_number
            .filter(|&n| matches.iter().any(|m| m.number == n))
            .unwrap_or(matches[0].number);

        let keeper = matches.iter().find(|m| m.number == keeper_number).cloned().expect("keeper");
        let extras: Vec<_> = matches.into_iter().filter(|m| m.number != keeper_number).collect();

        plan.groups.push(DuplicateGroup {
            local_id: ticket.id.clone(),
            title: ticket.title.clone(),
            full_repo: ticket.full_repo.clone(),
            keeper,
            extras,
            preferred_missing,
        });
    }

    plan.groups.sort_by(|a, b| (&a.full_repo, &a.local_id).cmp(&(&b.full_repo, &b.local_id)));
    plan
}

async fn check_notion(report: &mut DoctorReport, config: &RoadmapConfig) {
    use crate::notion::NotionClient;

    match NotionClient::ensure_on_path() {
        Ok(()) => report.checks.push(DoctorReport::pass("ntn", "found on PATH")),
        Err(err) => {
            report.checks.push(DoctorReport::fail("ntn", err.to_string()));
            return;
        }
    }

    let client = NotionClient::new();
    match client.auth_status().await {
        Ok(who) => {
            report.checks.push(DoctorReport::pass("ntn auth", format!("logged in as {who}")));
        }
        Err(err) => {
            report.checks.push(DoctorReport::fail("ntn auth", err.to_string()));
            return;
        }
    }

    let Ok(notion) = config.notion_config() else {
        report
            .checks
            .push(DoctorReport::fail("notion config", "enabled but invalid [platform.notion]"));
        return;
    };

    match client.get_data_source(&notion.data_source_id).await {
        Ok(_) => report.checks.push(DoctorReport::pass(
            "notion data source",
            format!("can read data source {}", notion.data_source_id),
        )),
        Err(err) => report.checks.push(DoctorReport::fail("notion data source", err.to_string())),
    }
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
            let repos: Vec<_> =
                github.repos.values().map(|slug| format!("{}/{}", github.owner, slug)).collect();
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
        if config.notion_enabled() {
            eprintln!();
            eprintln!("Notion:");
            eprintln!("  ntn login");
            eprintln!("  # or: export NOTION_API_TOKEN=…");
            eprintln!("  roadmap setup --notion-only");
        }
    }
}

pub fn print_dedup_plan(plan: &DedupPlan, apply: bool) {
    for warning in &plan.warnings {
        println!("{} {warning}", style::status_fail());
    }
    if plan.groups.is_empty() {
        if plan.warnings.is_empty() {
            println!("{}", style::ok("dedup: no duplicates found"));
        }
        return;
    }

    println!();
    println!("{} {} title group(s):", style::heading("dedup:"), plan.groups.len());
    for group in &plan.groups {
        println!();
        println!("  {} ({}) — {}", style::emphasis(&group.local_id), group.full_repo, group.title);
        println!("    keep  #{} {}", group.keeper.number, group.keeper.url);
        if group.preferred_missing {
            println!("    note  preferred keeper not open; kept lowest matching number");
        }
        for extra in &group.extras {
            let action = if apply { "closed" } else { "close " };
            println!("    {action} #{} {}", extra.number, extra.url);
        }
    }
    if !apply {
        println!();
        println!("Re-run with `roadmap doctor --dedup --yes` to close the extras.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(id: &str, title: &str, repo: &str, preferred: Option<u64>) -> LocalTicket {
        LocalTicket {
            id: id.into(),
            title: title.into(),
            full_repo: repo.into(),
            preferred_number: preferred,
        }
    }

    fn remote(number: u64, title: &str) -> RemoteIssueBrief {
        RemoteIssueBrief {
            number,
            title: title.into(),
            url: format!("https://github.com/o/r/issues/{number}"),
        }
    }

    #[test]
    fn no_duplicates_when_single_match() {
        let locals = vec![ticket("app-a", "Alpha", "acme/app", Some(1))];
        let remotes =
            HashMap::from([("acme/app".into(), vec![remote(1, "Alpha"), remote(2, "Other")])]);
        let plan = find_duplicate_groups(&locals, &remotes);
        assert_eq!(plan.groups, []);
    }

    #[test]
    fn prefers_sync_map_number_as_keeper() {
        let locals = vec![ticket("app-a", "Alpha", "acme/app", Some(81))];
        let remotes = HashMap::from([(
            "acme/app".into(),
            vec![remote(80, "Alpha"), remote(81, "Alpha"), remote(82, "Alpha")],
        )]);
        let plan = find_duplicate_groups(&locals, &remotes);
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(plan.groups[0].keeper.number, 81);
        assert_eq!(
            plan.groups[0].extras.iter().map(|e| e.number).collect::<Vec<_>>(),
            vec![80, 82]
        );
        assert!(!plan.groups[0].preferred_missing);
    }

    #[test]
    fn falls_back_to_lowest_number_when_preferred_missing() {
        let locals = vec![ticket("app-a", "Alpha", "acme/app", Some(99))];
        let remotes =
            HashMap::from([("acme/app".into(), vec![remote(82, "Alpha"), remote(80, "Alpha")])]);
        let plan = find_duplicate_groups(&locals, &remotes);
        assert_eq!(plan.groups[0].keeper.number, 80);
        assert!(plan.groups[0].preferred_missing);
        assert_eq!(plan.groups[0].extras[0].number, 82);
    }

    #[test]
    fn skips_ambiguous_local_titles() {
        let locals = vec![
            ticket("app-a", "Same", "acme/app", Some(1)),
            ticket("app-b", "Same", "acme/app", Some(2)),
        ];
        let remotes = HashMap::from([(
            "acme/app".into(),
            vec![remote(1, "Same"), remote(2, "Same"), remote(3, "Same")],
        )]);
        let plan = find_duplicate_groups(&locals, &remotes);
        assert_eq!(plan.groups, []);
        assert_eq!(plan.warnings.len(), 1);
        assert!(plan.warnings[0].contains("app-a"));
        assert!(plan.warnings[0].contains("app-b"));
    }
}
