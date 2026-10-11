use crate::error::{Error, Result};
use crate::model::{Roadmap, SyncMap};
use crate::style;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotionAction {
    CreatePage,
    UpdatePage,
}

impl NotionAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::CreatePage => "CreatePage",
            Self::UpdatePage => "UpdatePage",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NotionJob {
    pub id: String,
    pub title: String,
    pub action: NotionAction,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct NotionPlan {
    pub jobs: Vec<NotionJob>,
}

impl NotionPlan {
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }
}

pub fn build_notion_plan(roadmap: &Roadmap) -> Result<NotionPlan> {
    let meta = roadmap.sync_map.meta.notion.as_ref().ok_or_else(|| {
        Error::Config(
            "Notion property mapping missing in sync-map (_meta.notion); run `roadmap setup --notion-only`"
                .into(),
        )
    })?;
    if meta.properties.title.name.is_empty() {
        return Err(Error::Config("Notion title property mapping is required".into()));
    }

    let mut jobs = Vec::new();
    for milestone in &roadmap.milestones {
        let page_id = notion_page_id(&roadmap.sync_map, milestone.id());
        let (action, detail) = match page_id {
            Some(id) => (NotionAction::UpdatePage, format!("page {id}")),
            None => (NotionAction::CreatePage, format!("database {}", meta.database_id)),
        };
        jobs.push(NotionJob {
            id: milestone.id().to_string(),
            title: milestone.file.frontmatter.title.clone(),
            action,
            detail,
        });
    }
    Ok(NotionPlan { jobs })
}

pub fn notion_page_id(sync_map: &SyncMap, milestone_id: &str) -> Option<String> {
    sync_map.issue_entry(milestone_id).and_then(|entry| entry.notion_page_id.clone())
}

pub fn print_notion_plan(plan: &NotionPlan) {
    if plan.is_empty() {
        println!("notion plan: no jobs");
        return;
    }
    println!("notion plan: {} milestone job(s)", plan.job_count());
    println!();
    for (idx, job) in plan.jobs.iter().enumerate() {
        println!(
            "{} {} {}",
            style::job_index(idx + 1, plan.job_count()),
            style::emphasis(job.action.label()),
            job.id
        );
        println!("  {}", job.title);
        println!("  {}", job.detail);
        println!();
    }
}
