use async_trait::async_trait;
use std::collections::HashMap;

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IssueRef {
    pub repo: String,
    pub number: u64,
}

#[derive(Debug, Clone)]
pub struct RemoteIssue {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub body: String,
    pub state: String,
    pub parent_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RemoteIssueSnapshot {
    pub issue: RemoteIssue,
    pub field_values: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ProjectIssueItem {
    pub issue_url: String,
    pub item_id: String,
    pub field_values: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ProjectMeta {
    pub project_id: String,
    pub fields: Vec<ProjectField>,
}

#[derive(Debug, Clone)]
pub struct ProjectField {
    pub id: String,
    pub name: String,
    pub field_type: String,
    pub options: Vec<ProjectFieldOption>,
}

#[derive(Debug, Clone)]
pub struct ProjectFieldOption {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct IssueFieldDefinition {
    pub id: String,
    pub name: String,
    pub data_type: String,
    pub options: Vec<IssueFieldOption>,
}

#[derive(Debug, Clone)]
pub struct IssueFieldOption {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct IssueFieldValueUpdate {
    pub field_name: String,
    pub value: Option<String>,
}

impl ProjectMeta {
    pub fn field_by_name(&self, name: &str) -> Option<&ProjectField> {
        self.fields.iter().find(|field| field.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct CreateIssueRequest {
    pub repo: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub parent_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UpdateIssueRequest {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    #[allow(dead_code)] // reserved for label merge on update
    pub labels: Vec<String>,
    pub state: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectRef {
    pub owner: String,
    pub number: u32,
}

#[derive(Debug, Clone)]
pub struct ProjectItem {
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct SetProjectStatusRequest {
    pub item_id: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct SetProjectFieldValueRequest {
    pub item_id: String,
    pub field_id: String,
    /// `None` or empty clears the field value.
    pub value: Option<String>,
    /// Override when project field-list returns an empty `dataType`.
    pub field_type: Option<String>,
}

/// Backend-agnostic interface for issue trackers and project boards.
#[async_trait]
pub trait Platform: Send + Sync {
    async fn create_issue(&self, request: CreateIssueRequest) -> Result<RemoteIssue>;

    async fn update_issue(&self, request: UpdateIssueRequest) -> Result<RemoteIssue>;

    async fn add_issue_to_project(
        &self,
        project: &ProjectRef,
        issue_url: &str,
    ) -> Result<ProjectItem>;

    async fn set_project_item_status(
        &self,
        project: &ProjectRef,
        request: SetProjectStatusRequest,
    ) -> Result<()>;

    async fn link_parent(&self, repo: &str, issue_number: u64, parent_url: &str) -> Result<()>;

    async fn get_issue(&self, issue_ref: &IssueRef) -> Result<RemoteIssue>;

    async fn get_issue_field_values(&self, issue_ref: &IssueRef)
        -> Result<HashMap<String, String>>;

    async fn get_issues_snapshot(
        &self,
        repo: &str,
        issue_numbers: &[u64],
    ) -> Result<HashMap<u64, RemoteIssueSnapshot>>;

    async fn list_issue_fields(&self, repo: &str) -> Result<Vec<IssueFieldDefinition>>;

    async fn set_issue_field_values(
        &self,
        issue_ref: &IssueRef,
        updates: &[IssueFieldValueUpdate],
        fields: &[IssueFieldDefinition],
    ) -> Result<()>;

    async fn list_project_items(&self, project: &ProjectRef) -> Result<Vec<ProjectIssueItem>>;

    async fn load_project_meta(&self, project: &ProjectRef) -> Result<ProjectMeta>;

    async fn set_project_item_status_with_meta(
        &self,
        project: &ProjectRef,
        request: SetProjectStatusRequest,
        meta: &ProjectMeta,
    ) -> Result<()>;

    async fn set_project_item_field_value_with_meta(
        &self,
        project: &ProjectRef,
        request: SetProjectFieldValueRequest,
        meta: &ProjectMeta,
    ) -> Result<()>;
}

pub mod gh;

use gh::GhPlatform;

pub fn from_config(config: &crate::config::RoadmapConfig) -> Result<Box<dyn Platform>> {
    let github = config.github()?;
    Ok(Box::new(GhPlatform::new(github.host.clone(), github.owner.clone(), github.project_number)))
}
