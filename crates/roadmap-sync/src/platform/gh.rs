use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use reqwest::{Client, Method, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;
use tracing::{debug, warn};

use crate::error::{PlatformError, Result};
use crate::platform::{
    CreateIssueRequest, IssueFieldDefinition, IssueFieldOption, IssueFieldValueUpdate, IssueRef,
    Platform, ProjectField, ProjectFieldOption, ProjectIssueItem, ProjectItem, ProjectMeta,
    ProjectRef, RemoteIssue, RemoteIssueSnapshot, SetProjectFieldValueRequest,
    SetProjectStatusRequest, UpdateIssueRequest,
};

const PAT_HINT: &str = "GitHub token lacks permission. Fine-grained PAT: grant Issues \
                          Read and write on each repo, and Projects Read and write on the \
                          owner org/user. Or run: gh auth refresh -h github.com -s repo,project";

#[derive(Debug)]
pub struct GhPlatform {
    rest_api_url: String,
    graphql_url: String,
    http_client: Client,
    auth_token: Mutex<Option<String>>,
}

impl GhPlatform {
    pub fn new(host: String, owner: String, project_number: u32) -> Self {
        let (rest_api_url, graphql_url) = if host == "github.com" {
            ("https://api.github.com".to_string(), "https://api.github.com/graphql".to_string())
        } else {
            (format!("https://{host}/api/v3"), format!("https://{host}/api/graphql"))
        };
        let auth_token =
            std::env::var("GH_TOKEN").ok().or_else(|| std::env::var("GITHUB_TOKEN").ok());
        let _ = (owner, project_number);
        Self {
            rest_api_url,
            graphql_url,
            http_client: Client::new(),
            auth_token: Mutex::new(auth_token),
        }
    }

    pub async fn auth_status(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct User {
            login: String,
        }
        let user: User = self.run_json(&["api", "user"]).await?;
        Ok(user.login)
    }

    pub async fn repo_access(&self, full_repo: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Repo {
            permissions: Option<Permissions>,
        }
        #[derive(Deserialize)]
        struct Permissions {
            admin: Option<bool>,
            push: Option<bool>,
            pull: Option<bool>,
        }

        let path = format!("repos/{full_repo}");
        let repo: Repo = self.run_json(&["api", &path]).await?;
        let perms = repo.permissions.unwrap_or(Permissions { admin: None, push: None, pull: None });
        Ok(format!("admin={:?} push={:?} pull={:?}", perms.admin, perms.push, perms.pull))
    }

    /// Dry permission probe — creates and immediately closes a throwaway issue.
    pub async fn probe_issue_write(&self, full_repo: &str) -> Result<()> {
        let path = format!("repos/{full_repo}/issues");
        let body = serde_json::json!({
            "title": "__roadmap_doctor_probe__",
            "body": "automated permission probe — safe to close",
        });
        let value = self.api_json_raw("POST", &path, body).await?;
        if let Some(number) = value.get("number").and_then(serde_json::Value::as_u64) {
            let close_path = format!("repos/{full_repo}/issues/{number}");
            let _ = self
                .api_json_raw("PATCH", &close_path, serde_json::json!({ "state": "closed" }))
                .await;
        }
        Ok(())
    }

    pub async fn probe_project_write(&self, project: &ProjectRef) -> Result<()> {
        let query = "query($org: String!, $number: Int!) { organization(login: $org) { projectV2(number: $number) { viewerCanUpdate } } }";
        let response = self
            .api_graphql(
                query,
                serde_json::json!({
                    "org": project.owner,
                    "number": project.number,
                }),
            )
            .await?;

        let can_update = response
            .pointer("/data/organization/projectV2/viewerCanUpdate")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        if can_update {
            Ok(())
        } else {
            Err(command_failed(
                "gh",
                &format!(
                    "token cannot update project #{} for org {}",
                    project.number, project.owner
                ),
            ))
        }
    }

    async fn fetch_project_fields_graphql(&self, project_id: &str) -> Result<Vec<ProjectField>> {
        const QUERY: &str = r"
            query($id: ID!) {
              node(id: $id) {
                ... on ProjectV2 {
                  fields(first: 100) {
                    nodes {
                      __typename
                      ... on ProjectV2FieldCommon {
                        id
                        name
                        dataType
                      }
                      ... on ProjectV2SingleSelectField {
                        options { id name }
                      }
                    }
                  }
                }
              }
            }";
        let response = self.api_graphql(QUERY, serde_json::json!({ "id": project_id })).await?;
        let nodes = response
            .pointer("/data/node/fields/nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| command_failed("gh", "graphql: missing project fields nodes"))?;

        let mut fields = Vec::new();
        for node in nodes {
            let id = node.get("id").and_then(Value::as_str).unwrap_or_default();
            let name = node.get("name").and_then(Value::as_str).unwrap_or_default();
            if id.is_empty() || name.is_empty() {
                continue;
            }
            let data_type = node.get("dataType").and_then(Value::as_str).unwrap_or_default();
            let type_name = node.get("__typename").and_then(Value::as_str).unwrap_or_default();
            let field_type = normalize_project_data_type(data_type, type_name, id);
            let options = node
                .get("options")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .filter_map(|option| {
                            let id = option.get("id").and_then(Value::as_str)?;
                            let name = option.get("name").and_then(Value::as_str)?;
                            Some(ProjectFieldOption { id: id.to_string(), name: name.to_string() })
                        })
                        .collect()
                })
                .unwrap_or_default();
            fields.push(ProjectField {
                id: id.to_string(),
                name: name.to_string(),
                field_type,
                options,
            });
        }
        Ok(fields)
    }

    async fn run_output(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("gh")
            .args(args)
            .output()
            .await
            .map_err(|e| command_failed("gh", &format!("failed to spawn: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        if !output.status.success() {
            return Err(command_failed("gh", &enrich_error(&stderr)));
        }

        Ok(stdout)
    }

    async fn run_json<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> Result<T> {
        let stdout = self.run_output(args).await?;
        serde_json::from_str(&stdout)
            .map_err(|source| PlatformError::InvalidJson { program: "gh".into(), source }.into())
    }

    async fn auth_token(&self) -> Result<String> {
        if let Some(token) = self.auth_token.lock().expect("auth token mutex poisoned").clone() {
            return Ok(token);
        }
        let token = self.run_output(&["auth", "token"]).await?;
        let token = token.trim().to_string();
        let mut guard = self.auth_token.lock().expect("auth token mutex poisoned");
        if let Some(cached) = guard.clone() {
            return Ok(cached);
        }
        *guard = Some(token.clone());
        Ok(token)
    }

    async fn api_json_raw(&self, method: &str, path: &str, body: Value) -> Result<Value> {
        let token = self.auth_token().await?;
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|e| command_failed("gh", &format!("invalid HTTP method `{method}`: {e}")))?;
        let url = format!("{}/{}", self.rest_api_url, path.trim_start_matches('/'));

        let response = self
            .http_client
            .request(method, &url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "roadmap-sync")
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| command_failed("gh", &format!("HTTP request failed: {e}")))?;

        Self::response_to_json(response).await
    }

    async fn api_json<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Value,
    ) -> Result<T> {
        let value = self.api_json_raw(method, path, body).await?;
        serde_json::from_value(value)
            .map_err(|source| PlatformError::InvalidJson { program: "gh".into(), source }.into())
    }

    async fn api_graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let token = self.auth_token().await?;
        let payload = serde_json::json!({ "query": query, "variables": variables });
        let response = self
            .http_client
            .post(&self.graphql_url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "roadmap-sync")
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| command_failed("gh", &format!("GraphQL request failed: {e}")))?;
        let response = Self::response_to_json(response).await?;

        if let Some(errors) = response.get("errors") {
            return Err(command_failed("gh", &format!("graphql errors: {errors}")));
        }

        Ok(response)
    }

    async fn response_to_json(response: reqwest::Response) -> Result<Value> {
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| command_failed("gh", &format!("failed reading HTTP response: {e}")))?;
        if !status.is_success() {
            let msg = if status == StatusCode::FORBIDDEN {
                enrich_error(&format!("HTTP {}: {text}", status.as_u16()))
            } else {
                format!("HTTP {}: {text}", status.as_u16())
            };
            return Err(command_failed("gh", &msg));
        }
        serde_json::from_str::<Value>(&text).map_err(|source| {
            PlatformError::InvalidJson { program: "github api".into(), source }.into()
        })
    }

    async fn create_with_body_file(&self, request: &CreateIssueRequest) -> Result<RemoteIssue> {
        let temp = tempfile::NamedTempFile::new()
            .map_err(|e| command_failed("gh", &format!("failed to create temp body file: {e}")))?;
        std::fs::write(temp.path(), &request.body)
            .map_err(|e| command_failed("gh", &format!("failed to write temp body file: {e}")))?;
        let body_path = temp.path().to_string_lossy().into_owned();

        let mut args = vec![
            "issue",
            "create",
            "-R",
            &request.repo,
            "--title",
            &request.title,
            "--body-file",
            body_path.as_str(),
        ];
        for label in &request.labels {
            args.push("--label");
            args.push(label);
        }
        if let Some(parent) = &request.parent_url {
            args.push("--parent");
            args.push(parent);
        }

        match self.run_output(&args).await {
            Ok(url) => parse_issue_url(&url),
            Err(err) => {
                if request.labels.is_empty() {
                    return Err(err);
                }
                warn!("issue create with labels failed; retrying without labels");
                let args = [
                    "issue",
                    "create",
                    "-R",
                    &request.repo,
                    "--title",
                    &request.title,
                    "--body-file",
                    &body_path,
                ];
                let url = self.run_output(&args).await?;
                parse_issue_url(&url)
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhApiIssueResponse {
    number: u64,
    html_url: String,
    title: String,
    body: Option<String>,
    state: String,
}

#[derive(Debug, Deserialize)]
struct GhIssueViewResponse {
    number: u64,
    url: String,
    title: String,
    body: String,
    state: String,
    #[serde(default)]
    parent: Option<GhIssueParent>,
}

#[derive(Debug, Deserialize)]
struct GhIssueParent {
    url: String,
}

#[derive(Debug, Deserialize)]
struct GhProjectItemResponse {
    id: String,
}

#[derive(Debug, Deserialize)]
struct GhProjectView {
    id: String,
}

#[derive(Debug, Deserialize)]
struct GhProjectFields {
    fields: Vec<GhProjectField>,
}

#[derive(Debug, Deserialize)]
struct GhProjectField {
    id: String,
    name: String,
    /// GraphQL typename on older `gh` (`ProjectV2Field`, …).
    #[serde(rename = "type", default)]
    type_name: String,
    /// Present on newer `gh` (`NUMBER`, `SINGLE_SELECT`, …).
    #[serde(rename = "dataType", default)]
    data_type: String,
    #[serde(default)]
    options: Vec<GhProjectFieldOption>,
}

impl GhProjectField {
    fn resolved_data_type(&self) -> String {
        normalize_project_data_type(&self.data_type, &self.type_name, &self.id)
    }
}

#[derive(Debug, Deserialize)]
struct GhProjectFieldOption {
    id: String,
    name: String,
}

#[async_trait]
impl Platform for GhPlatform {
    async fn create_issue(&self, request: CreateIssueRequest) -> Result<RemoteIssue> {
        debug!(repo = %request.repo, title = %request.title, "creating issue");
        self.create_with_body_file(&request).await
    }

    async fn update_issue(&self, request: UpdateIssueRequest) -> Result<RemoteIssue> {
        debug!(
            repo = %request.repo,
            number = request.number,
            "updating issue via gh api"
        );
        let path = format!("repos/{}/issues/{}", request.repo, request.number);
        let mut body = serde_json::json!({
            "title": request.title,
            "body": request.body,
        });
        if let Some(state) = &request.state {
            body["state"] = Value::String(state.clone());
        }
        let response: GhApiIssueResponse = self.api_json("PATCH", &path, body).await?;
        Ok(response.into_remote())
    }

    async fn get_issue(&self, issue_ref: &IssueRef) -> Result<RemoteIssue> {
        debug!(repo = %issue_ref.repo, number = issue_ref.number, "fetching issue");
        let number = issue_ref.number.to_string();
        let args = [
            "issue",
            "view",
            &number,
            "-R",
            &issue_ref.repo,
            "--json",
            "number,title,body,state,url,parent",
        ];
        let response: GhIssueViewResponse = self.run_json(&args).await?;
        Ok(response.into_remote())
    }

    async fn get_issue_field_values(
        &self,
        issue_ref: &IssueRef,
    ) -> Result<HashMap<String, String>> {
        debug!(
            repo = %issue_ref.repo,
            number = issue_ref.number,
            "fetching issue custom field values"
        );
        let (owner, repo) = issue_ref.repo.split_once('/').ok_or_else(|| {
            command_failed(
                "gh",
                &format!("invalid issue repo `{}` (expected owner/repo)", issue_ref.repo),
            )
        })?;
        const QUERY: &str = r"
            query($owner: String!, $repo: String!, $number: Int!) {
              repository(owner: $owner, name: $repo) {
                issue(number: $number) {
                  issueFieldValues(first: 50) {
                    nodes {
                      __typename
                      ... on IssueFieldDateValue {
                        value
                        field { ... on IssueFieldDate { name } }
                      }
                      ... on IssueFieldSingleSelectValue {
                        name
                        value
                        field { ... on IssueFieldSingleSelect { name } }
                      }
                      ... on IssueFieldMultiSelectValue {
                        value
                        field { ... on IssueFieldMultiSelect { name } }
                      }
                      ... on IssueFieldTextValue {
                        value
                        field { ... on IssueFieldText { name } }
                      }
                      ... on IssueFieldNumberValue {
                        value
                        field { ... on IssueFieldNumber { name } }
                      }
                    }
                  }
                }
              }
            }";

        let response = self
            .api_graphql(
                QUERY,
                serde_json::json!({
                    "owner": owner,
                    "repo": repo,
                    "number": issue_ref.number,
                }),
            )
            .await?;
        let nodes = response
            .pointer("/data/repository/issue/issueFieldValues/nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| command_failed("gh", "graphql: missing issueFieldValues nodes"))?;

        Ok(parse_issue_field_values(nodes))
    }

    async fn get_issues_snapshot(
        &self,
        repo: &str,
        issue_numbers: &[u64],
    ) -> Result<HashMap<u64, RemoteIssueSnapshot>> {
        if issue_numbers.is_empty() {
            return Ok(HashMap::new());
        }

        let (owner, repo_name) = repo.split_once('/').ok_or_else(|| {
            command_failed("gh", &format!("invalid issue repo `{repo}` (expected owner/repo)"))
        })?;
        let mut snapshots = HashMap::new();

        for chunk in issue_numbers.chunks(25) {
            let owner_lit = serde_json::to_string(owner)
                .map_err(|source| PlatformError::InvalidJson { program: "gh".into(), source })?;
            let repo_lit = serde_json::to_string(repo_name)
                .map_err(|source| PlatformError::InvalidJson { program: "gh".into(), source })?;

            let mut query = format!("query {{ repository(owner: {owner_lit}, name: {repo_lit}) {{");
            for number in chunk {
                query.push_str(&format!(
                    " issue_{number}: issue(number: {number}) {{ number url title body state parent {{ url }} issueFieldValues(first: 50) {{ nodes {{ __typename ... on IssueFieldDateValue {{ value field {{ ... on IssueFieldDate {{ name }} }} }} ... on IssueFieldSingleSelectValue {{ name value field {{ ... on IssueFieldSingleSelect {{ name }} }} }} ... on IssueFieldMultiSelectValue {{ value field {{ ... on IssueFieldMultiSelect {{ name }} }} }} ... on IssueFieldTextValue {{ value field {{ ... on IssueFieldText {{ name }} }} }} ... on IssueFieldNumberValue {{ value field {{ ... on IssueFieldNumber {{ name }} }} }} }} }} }}"
                ));
            }
            query.push_str(" } }");

            let response = self.api_graphql(&query, serde_json::json!({})).await?;
            let repo_obj = response
                .pointer("/data/repository")
                .and_then(Value::as_object)
                .ok_or_else(|| command_failed("gh", "graphql: missing repository object"))?;

            for number in chunk {
                let alias = format!("issue_{number}");
                let issue_node = repo_obj
                    .get(&alias)
                    .ok_or_else(|| command_failed("gh", &format!("graphql: missing `{alias}`")))?;
                if issue_node.is_null() {
                    continue;
                }
                let number = issue_node
                    .get("number")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| command_failed("gh", "graphql: issue missing number"))?;
                let url = issue_node
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| command_failed("gh", "graphql: issue missing url"))?
                    .to_string();
                let title =
                    issue_node.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
                let body =
                    issue_node.get("body").and_then(Value::as_str).unwrap_or_default().to_string();
                let state = issue_node
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase();
                let parent_url =
                    issue_node.pointer("/parent/url").and_then(Value::as_str).map(str::to_string);
                let field_values = issue_node
                    .pointer("/issueFieldValues/nodes")
                    .and_then(Value::as_array)
                    .map(|nodes| parse_issue_field_values(nodes.as_slice()))
                    .unwrap_or_default();

                snapshots.insert(
                    number,
                    RemoteIssueSnapshot {
                        issue: RemoteIssue { number, url, title, body, state, parent_url },
                        field_values,
                    },
                );
            }
        }

        Ok(snapshots)
    }

    async fn list_issue_fields(&self, repo: &str) -> Result<Vec<IssueFieldDefinition>> {
        let (owner, repo_name) = repo.split_once('/').ok_or_else(|| {
            command_failed("gh", &format!("invalid repo `{repo}` (expected owner/repo)"))
        })?;
        const QUERY: &str = r"
            query($owner: String!, $repo: String!) {
              repository(owner: $owner, name: $repo) {
                issueFields(first: 100) {
                  nodes {
                    __typename
                    ... on IssueFieldDate {
                      id
                      name
                      dataType
                    }
                    ... on IssueFieldText {
                      id
                      name
                      dataType
                    }
                    ... on IssueFieldNumber {
                      id
                      name
                      dataType
                    }
                    ... on IssueFieldSingleSelect {
                      id
                      name
                      dataType
                      options {
                        id
                        name
                      }
                    }
                    ... on IssueFieldMultiSelect {
                      id
                      name
                      dataType
                      options {
                        id
                        name
                      }
                    }
                  }
                }
              }
            }";
        let response = self
            .api_graphql(
                QUERY,
                serde_json::json!({
                    "owner": owner,
                    "repo": repo_name,
                }),
            )
            .await?;
        let nodes = response
            .pointer("/data/repository/issueFields/nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| command_failed("gh", "graphql: missing issueFields nodes"))?;
        let mut fields = Vec::new();
        for node in nodes {
            let id = node.get("id").and_then(Value::as_str).map(str::to_string);
            let name = node.get("name").and_then(Value::as_str).map(str::to_string);
            let data_type = node.get("dataType").and_then(Value::as_str).map(str::to_string);
            let (Some(id), Some(name), Some(data_type)) = (id, name, data_type) else {
                continue;
            };
            let options = node
                .get("options")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|option| {
                            let id = option.get("id").and_then(Value::as_str)?;
                            let name = option.get("name").and_then(Value::as_str)?;
                            Some(IssueFieldOption { id: id.to_string(), name: name.to_string() })
                        })
                        .collect()
                })
                .unwrap_or_default();
            fields.push(IssueFieldDefinition { id, name, data_type, options });
        }
        Ok(fields)
    }

    async fn set_issue_field_values(
        &self,
        issue_ref: &IssueRef,
        updates: &[IssueFieldValueUpdate],
        fields: &[IssueFieldDefinition],
    ) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let (owner, repo_name) = issue_ref
            .repo
            .split_once('/')
            .ok_or_else(|| command_failed("gh", &format!("invalid repo `{}`", issue_ref.repo)))?;
        const ISSUE_QUERY: &str = r"
            query($owner: String!, $repo: String!, $number: Int!) {
              repository(owner: $owner, name: $repo) {
                issue(number: $number) { id }
              }
            }";
        let issue_data = self
            .api_graphql(
                ISSUE_QUERY,
                serde_json::json!({
                    "owner": owner,
                    "repo": repo_name,
                    "number": issue_ref.number,
                }),
            )
            .await?;
        let issue_id = issue_data
            .pointer("/data/repository/issue/id")
            .and_then(Value::as_str)
            .ok_or_else(|| command_failed("gh", "graphql: missing issue id"))?;

        let mut issue_fields = Vec::new();
        for update in updates {
            let Some(field) = fields.iter().find(|field| field.name == update.field_name) else {
                continue;
            };
            let mut payload = serde_json::json!({
                "fieldId": field.id,
            });
            let value = update.value.as_deref().filter(|value| !value.is_empty());
            match (field.data_type.as_str(), value) {
                (_, None) => {
                    payload["delete"] = Value::Bool(true);
                }
                ("DATE", Some(value)) => {
                    payload["dateValue"] = Value::String(value.to_string());
                }
                ("TEXT", Some(value)) => {
                    payload["textValue"] = Value::String(value.to_string());
                }
                ("NUMBER", Some(value)) => {
                    let number = value.parse::<f64>().map_err(|_| {
                        command_failed(
                            "gh",
                            &format!("invalid number `{value}` for issue field `{}`", field.name),
                        )
                    })?;
                    payload["numberValue"] = serde_json::json!(number);
                }
                ("SINGLE_SELECT", Some(value)) => {
                    let option =
                        field.options.iter().find(|option| option.name == value).ok_or_else(
                            || {
                                command_failed(
                                    "gh",
                                    &format!(
                                        "issue field `{}` has no single-select option `{value}`",
                                        field.name
                                    ),
                                )
                            },
                        )?;
                    payload["singleSelectOptionId"] = Value::String(option.id.clone());
                }
                ("MULTI_SELECT", Some(value)) => {
                    let wanted: Vec<&str> =
                        value.split(',').map(str::trim).filter(|item| !item.is_empty()).collect();
                    let mut option_ids = Vec::new();
                    for wanted_name in wanted {
                        let option = field
                            .options
                            .iter()
                            .find(|option| option.name == wanted_name)
                            .ok_or_else(|| {
                                command_failed(
                                    "gh",
                                    &format!(
                                        "issue field `{}` has no multi-select option `{wanted_name}`",
                                        field.name
                                    ),
                                )
                            })?;
                        option_ids.push(Value::String(option.id.clone()));
                    }
                    payload["multiSelectOptionIds"] = Value::Array(option_ids);
                }
                (other, Some(_)) => {
                    return Err(command_failed(
                        "gh",
                        &format!("unsupported issue field type `{other}`"),
                    ));
                }
            }
            issue_fields.push(payload);
        }

        if issue_fields.is_empty() {
            return Ok(());
        }

        const MUTATION: &str = r"
            mutation($input: SetIssueFieldValueInput!) {
              setIssueFieldValue(input: $input) {
                clientMutationId
              }
            }";
        let _ = self
            .api_graphql(
                MUTATION,
                serde_json::json!({
                    "input": {
                        "issueId": issue_id,
                        "issueFields": issue_fields,
                    }
                }),
            )
            .await?;
        Ok(())
    }

    async fn list_project_items(&self, project: &ProjectRef) -> Result<Vec<ProjectIssueItem>> {
        debug!(
            owner = %project.owner,
            number = project.number,
            "listing project items via graphql"
        );

        const QUERY: &str = r"
            query($org: String!, $number: Int!, $after: String) {
              organization(login: $org) {
                projectV2(number: $number) {
                  items(first: 100, after: $after) {
                    pageInfo { hasNextPage endCursor }
                    nodes {
                      id
                      content { ... on Issue { url } }
                      fieldValues(first: 20) {
                        nodes {
                          __typename
                          ... on ProjectV2ItemFieldDateValue {
                            date
                            field { ... on ProjectV2FieldCommon { id name } }
                          }
                          ... on ProjectV2ItemFieldSingleSelectValue {
                            name
                            field { ... on ProjectV2FieldCommon { id name } }
                          }
                          ... on ProjectV2ItemFieldTextValue {
                            text
                            field { ... on ProjectV2FieldCommon { id name } }
                          }
                          ... on ProjectV2ItemFieldNumberValue {
                            number
                            field { ... on ProjectV2FieldCommon { id name } }
                          }
                        }
                      }
                    }
                  }
                }
              }
            }";

        let mut items = Vec::new();
        let mut after: Option<String> = None;

        loop {
            let mut variables = serde_json::json!({
                "org": project.owner,
                "number": project.number,
            });
            if let Some(cursor) = &after {
                variables["after"] = Value::String(cursor.clone());
            }

            let response = self.api_graphql(QUERY, variables).await?;
            let nodes = response
                .pointer("/data/organization/projectV2/items/nodes")
                .and_then(Value::as_array)
                .ok_or_else(|| command_failed("gh", "graphql: missing project items nodes"))?;

            for node in nodes {
                let item_id = node
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| command_failed("gh", "graphql: project item missing id"))?
                    .to_string();

                let issue_url =
                    node.pointer("/content/url").and_then(Value::as_str).map(str::to_string);

                let Some(issue_url) = issue_url else {
                    continue;
                };

                let mut field_values = HashMap::new();

                if let Some(field_nodes) =
                    node.pointer("/fieldValues/nodes").and_then(Value::as_array)
                {
                    for field_node in field_nodes {
                        let Some(field_id) = field_node
                            .pointer("/field/id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                        else {
                            continue;
                        };

                        let typename = field_node
                            .get("__typename")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let value = match typename {
                            "ProjectV2ItemFieldSingleSelectValue" => {
                                field_node.get("name").and_then(Value::as_str).map(str::to_string)
                            }
                            "ProjectV2ItemFieldDateValue" => {
                                field_node.get("date").and_then(Value::as_str).map(str::to_string)
                            }
                            "ProjectV2ItemFieldTextValue" => {
                                field_node.get("text").and_then(Value::as_str).map(str::to_string)
                            }
                            "ProjectV2ItemFieldNumberValue" => field_node
                                .get("number")
                                .and_then(Value::as_f64)
                                .map(|n| n.to_string()),
                            _ => None,
                        };
                        if let Some(value) = value {
                            field_values.insert(field_id, value);
                        }
                    }
                }

                items.push(ProjectIssueItem { issue_url, item_id, field_values });
            }

            let page_info = response
                .pointer("/data/organization/projectV2/items/pageInfo")
                .ok_or_else(|| command_failed("gh", "graphql: missing pageInfo"))?;
            let has_next = page_info.get("hasNextPage").and_then(Value::as_bool).unwrap_or(false);
            if has_next {
                after = page_info.get("endCursor").and_then(Value::as_str).map(str::to_string);
            } else {
                break;
            }
        }

        Ok(items)
    }

    async fn load_project_meta(&self, project: &ProjectRef) -> Result<ProjectMeta> {
        let number = project.number.to_string();
        let project_view: GhProjectView = self
            .run_json(&["project", "view", &number, "--owner", &project.owner, "--format", "json"])
            .await?;

        // Prefer GraphQL `dataType` — older `gh project field-list --format json`
        // only exported the GraphQL typename as `type`, leaving dataType empty.
        match self.fetch_project_fields_graphql(&project_view.id).await {
            Ok(fields) if !fields.is_empty() => {
                return Ok(ProjectMeta { project_id: project_view.id, fields });
            }
            Ok(_) => {
                warn!("graphql project fields empty; falling back to gh field-list");
            }
            Err(err) => {
                warn!(error = %err, "graphql project fields failed; falling back to gh field-list");
            }
        }

        let fields: GhProjectFields = self
            .run_json(&[
                "project",
                "field-list",
                &number,
                "--owner",
                &project.owner,
                "--format",
                "json",
            ])
            .await?;

        Ok(ProjectMeta {
            project_id: project_view.id,
            fields: fields
                .fields
                .iter()
                .map(|field| ProjectField {
                    id: field.id.clone(),
                    name: field.name.clone(),
                    field_type: field.resolved_data_type(),
                    options: field
                        .options
                        .iter()
                        .map(|option| ProjectFieldOption {
                            id: option.id.clone(),
                            name: option.name.clone(),
                        })
                        .collect(),
                })
                .collect(),
        })
    }

    async fn set_project_item_status(
        &self,
        project: &ProjectRef,
        request: SetProjectStatusRequest,
    ) -> Result<()> {
        let meta = self.load_project_meta(project).await?;
        self.set_project_item_status_with_meta(project, request, &meta).await
    }

    async fn set_project_item_status_with_meta(
        &self,
        project: &ProjectRef,
        request: SetProjectStatusRequest,
        meta: &ProjectMeta,
    ) -> Result<()> {
        debug!(
            owner = %project.owner,
            number = project.number,
            item_id = %request.item_id,
            status = %request.status,
            "setting project item status"
        );

        let status_field = meta
            .field_by_name("Status")
            .ok_or_else(|| command_failed("gh", "project Status field not found"))?;
        let option =
            status_field.options.iter().find(|option| option.name == request.status).ok_or_else(
                || {
                    command_failed(
                        "gh",
                        &format!("project Status option `{}` not found", request.status),
                    )
                },
            )?;

        let args = [
            "project",
            "item-edit",
            "--id",
            &request.item_id,
            "--project-id",
            &meta.project_id,
            "--field-id",
            &status_field.id,
            "--single-select-option-id",
            &option.id,
        ];
        self.run_output(&args).await?;
        Ok(())
    }

    async fn set_project_item_field_value_with_meta(
        &self,
        project: &ProjectRef,
        request: SetProjectFieldValueRequest,
        meta: &ProjectMeta,
    ) -> Result<()> {
        let field = meta.fields.iter().find(|field| field.id == request.field_id);
        let field_name = field.map_or(request.field_id.as_str(), |field| field.name.as_str());
        let field_type = request
            .field_type
            .as_deref()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                field.map(|field| field.field_type.as_str()).filter(|value| !value.is_empty())
            })
            .unwrap_or("");
        let option_source = field;

        debug!(
            owner = %project.owner,
            number = project.number,
            item_id = %request.item_id,
            field_id = %request.field_id,
            field_name = %field_name,
            field_type = %field_type,
            "setting project item field value"
        );

        if field.is_none() && request.field_type.is_none() {
            return Err(command_failed(
                "gh",
                &format!("project field `{}` not found", request.field_id),
            ));
        }

        let value = request.value.as_deref().filter(|value| !value.is_empty());
        let mut args = vec![
            "project".to_string(),
            "item-edit".to_string(),
            "--id".to_string(),
            request.item_id.clone(),
            "--project-id".to_string(),
            meta.project_id.clone(),
            "--field-id".to_string(),
            request.field_id.clone(),
        ];

        let field_type_upper = field_type.to_ascii_uppercase();
        match (field_type_upper.as_str(), value) {
            (_, None) => {
                args.push("--clear".to_string());
            }
            ("NUMBER", Some(value)) => {
                let number: f64 = value.parse().map_err(|_| {
                    command_failed(
                        "gh",
                        &format!("invalid number `{value}` for project field `{field_name}`"),
                    )
                })?;
                args.push("--number".to_string());
                args.push(number.to_string());
            }
            ("TEXT", Some(value)) => {
                args.push("--text".to_string());
                args.push(value.to_string());
            }
            ("DATE", Some(value)) => {
                args.push("--date".to_string());
                args.push(value.to_string());
            }
            ("SINGLE_SELECT", Some(value)) => {
                let option = option_source
                    .and_then(|field| field.options.iter().find(|option| option.name == value))
                    .ok_or_else(|| {
                        command_failed(
                            "gh",
                            &format!(
                                "project field `{field_name}` has no single-select option `{value}`"
                            ),
                        )
                    })?;
                args.push("--single-select-option-id".to_string());
                args.push(option.id.clone());
            }
            (other, Some(_)) => {
                return Err(command_failed(
                    "gh",
                    &format!("unsupported project field type `{other}` for field `{field_name}`"),
                ));
            }
        }

        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run_output(&arg_refs).await?;
        Ok(())
    }

    async fn add_issue_to_project(
        &self,
        project: &ProjectRef,
        issue_url: &str,
    ) -> Result<ProjectItem> {
        debug!(
            owner = %project.owner,
            number = project.number,
            url = issue_url,
            "adding issue to project"
        );
        let number = project.number.to_string();
        let args = [
            "project",
            "item-add",
            &number,
            "--owner",
            &project.owner,
            "--url",
            issue_url,
            "--format",
            "json",
        ];
        let response: GhProjectItemResponse = self.run_json(&args).await?;
        Ok(ProjectItem { id: response.id })
    }

    async fn link_parent(&self, repo: &str, issue_number: u64, parent_url: &str) -> Result<()> {
        debug!(repo = %repo, issue_number, parent_url, "linking issue parent");
        let number = issue_number.to_string();
        let args = ["issue", "edit", &number, "-R", repo, "--parent", parent_url];
        match self.run_output(&args).await {
            Ok(_) => Ok(()),
            Err(err) if is_duplicate_sub_issue(&err) => {
                debug!(issue_number, parent_url, "sub-issue already linked to parent");
                Ok(())
            }
            Err(err) => Err(err),
        }
    }
}

impl GhApiIssueResponse {
    fn into_remote(self) -> RemoteIssue {
        RemoteIssue {
            number: self.number,
            url: self.html_url,
            title: self.title,
            body: self.body.unwrap_or_default(),
            state: self.state,
            parent_url: None,
        }
    }
}

impl GhIssueViewResponse {
    fn into_remote(self) -> RemoteIssue {
        RemoteIssue {
            number: self.number,
            url: self.url,
            title: self.title,
            body: self.body,
            state: self.state,
            parent_url: self.parent.map(|parent| parent.url),
        }
    }
}

/// Resolve a project field data type from `gh` JSON / GraphQL.
///
/// Older `gh project field-list` only exported GraphQL `__typename` as `type`
/// (e.g. `ProjectV2Field`) and omitted `dataType`.
fn normalize_project_data_type(data_type: &str, type_name: &str, field_id: &str) -> String {
    let direct = data_type.trim();
    if !direct.is_empty()
        && !direct.eq_ignore_ascii_case("ProjectV2Field")
        && !direct.starts_with("ProjectV2")
    {
        return direct.to_ascii_uppercase();
    }

    let type_name = type_name.trim();
    match type_name {
        "ProjectV2SingleSelectField" => return "SINGLE_SELECT".into(),
        "ProjectV2IterationField" => return "ITERATION".into(),
        "ProjectV2Field" => {}
        other
            if !other.is_empty()
                && !other.starts_with("ProjectV2")
                && other.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') =>
        {
            return other.to_ascii_uppercase();
        }
        _ => {}
    }

    // Node id prefixes from GitHub’s ProjectV2 field IDs.
    if field_id.starts_with("PVTSSF_") {
        return "SINGLE_SELECT".into();
    }
    if field_id.starts_with("PVTIF_") {
        return "ITERATION".into();
    }
    String::new()
}

fn parse_issue_field_values(nodes: &[Value]) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for node in nodes {
        let field_name = node.pointer("/field/name").and_then(Value::as_str).map(str::to_string);
        let value = node
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| node.get("value").and_then(Value::as_str))
            .map(str::to_string);
        if let (Some(field_name), Some(value)) = (field_name, value) {
            values.insert(field_name, value);
        }
    }
    values
}

fn parse_issue_url(stdout: &str) -> Result<RemoteIssue> {
    let url = stdout.lines().last().unwrap_or(stdout).trim();
    let Some((_, number)) = url.rsplit_once("/issues/") else {
        return Err(PlatformError::UnexpectedOutput {
            program: "gh issue create".into(),
            stdout: stdout.to_string(),
        }
        .into());
    };
    let number = number.parse::<u64>().map_err(|_| PlatformError::UnexpectedOutput {
        program: "gh issue create".into(),
        stdout: stdout.to_string(),
    })?;
    Ok(RemoteIssue {
        number,
        url: url.to_string(),
        title: String::new(),
        body: String::new(),
        state: "open".to_string(),
        parent_url: None,
    })
}

fn enrich_error(stderr: &str) -> String {
    if stderr.contains("Resource not accessible by personal access token")
        || stderr.contains("HTTP 403")
    {
        format!("{stderr}\n\n{PAT_HINT}")
    } else {
        stderr.to_string()
    }
}

fn is_duplicate_sub_issue(err: &crate::error::Error) -> bool {
    matches!(
        err,
        crate::error::Error::Platform(PlatformError::CommandFailed { message, .. })
            if message.contains("duplicate sub-issues")
    )
}

fn command_failed(program: &str, message: &str) -> crate::error::Error {
    PlatformError::CommandFailed { program: program.to_string(), message: message.to_string() }
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_response_maps_html_url() {
        let raw = r#"{
            "number": 42,
            "html_url": "https://github.com/o/r/issues/42",
            "title": "t",
            "body": "b",
            "state": "open"
        }"#;
        let issue: GhApiIssueResponse = serde_json::from_str(raw).unwrap();
        let remote = issue.into_remote();
        assert_eq!(remote.number, 42);
        assert_eq!(remote.url, "https://github.com/o/r/issues/42");
    }

    #[test]
    fn parses_issue_create_stdout() {
        let remote = parse_issue_url("https://github.com/example/app/issues/123\n").unwrap();
        assert_eq!(remote.number, 123);
        assert_eq!(remote.url, "https://github.com/example/app/issues/123");
    }

    #[test]
    fn enrich_error_adds_pat_hint() {
        let msg = enrich_error("gh: Resource not accessible by personal access token (HTTP 403)");
        assert!(msg.contains("Fine-grained PAT"));
    }

    #[test]
    fn detects_duplicate_sub_issue() {
        let err = command_failed(
            "gh",
            "GraphQL: Failed to add sub-issue #38 to parent #3. Issue may not contain duplicate sub-issues (addSubIssue)",
        );
        assert!(is_duplicate_sub_issue(&err));
    }

    #[test]
    fn normalizes_field_list_types() {
        assert_eq!(normalize_project_data_type("NUMBER", "ProjectV2Field", "PVTF_x"), "NUMBER");
        assert_eq!(
            normalize_project_data_type("", "ProjectV2SingleSelectField", "PVTF_x"),
            "SINGLE_SELECT"
        );
        assert_eq!(normalize_project_data_type("", "ProjectV2Field", "PVTSSF_x"), "SINGLE_SELECT");
        assert_eq!(normalize_project_data_type("", "ProjectV2Field", "PVTIF_x"), "ITERATION");
        assert_eq!(normalize_project_data_type("", "ProjectV2Field", "PVTF_x"), "");
    }
}
