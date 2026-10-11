use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;

use crate::error::{PlatformError, Result};

fn command_failed(message: impl Into<String>) -> crate::error::Error {
    PlatformError::CommandFailed { program: "ntn".into(), message: message.into() }.into()
}

#[derive(Debug, Default)]
pub struct NotionClient;

impl NotionClient {
    pub fn new() -> Self {
        Self
    }

    pub fn ensure_on_path() -> Result<()> {
        match std::process::Command::new("ntn").arg("--version").output() {
            Ok(out) if out.status.success() => Ok(()),
            Ok(_) | Err(_) => Err(crate::error::Error::Config(
                "`ntn` not found on PATH — install the Notion CLI: https://developers.notion.com/cli/get-started/overview"
                    .into(),
            )),
        }
    }

    pub async fn auth_status(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct Me {
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            id: Option<String>,
            #[serde(rename = "type", default)]
            user_type: Option<String>,
        }
        let me: Me = self.api_json("GET", "v1/users/me", None).await?;
        Ok(me.name.or(me.id).or(me.user_type).unwrap_or_else(|| "authenticated".into()))
    }

    /// Retrieve a data source (schema/properties). Notion API ≥ 2025-09-03.
    pub async fn get_data_source(&self, data_source_id: &str) -> Result<Value> {
        self.api_json("GET", &format!("v1/data_sources/{data_source_id}"), None).await
    }

    /// Search for data sources shared with the connection (replaces database search).
    pub async fn search_data_sources(&self) -> Result<Vec<NotionDataSource>> {
        let mut results = Vec::new();
        let mut start_cursor: Option<String> = None;
        loop {
            let mut body = serde_json::json!({
                "filter": { "property": "object", "value": "data_source" },
                "page_size": 100
            });
            if let Some(cursor) = &start_cursor {
                body["start_cursor"] = Value::String(cursor.clone());
            }
            let response: Value = self.api_json("POST", "v1/search", Some(body)).await?;
            let page_results =
                response.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
            for item in page_results {
                if let Some(ds) = NotionDataSource::from_value(&item) {
                    results.push(ds);
                }
            }
            let has_more = response.get("has_more").and_then(Value::as_bool).unwrap_or(false);
            if !has_more {
                break;
            }
            start_cursor = response.get("next_cursor").and_then(Value::as_str).map(str::to_string);
            if start_cursor.is_none() {
                break;
            }
        }
        results.sort_by(|a, b| a.title.cmp(&b.title));
        Ok(results)
    }

    pub async fn create_page(
        &self,
        parent_data_source_id: &str,
        properties: Value,
    ) -> Result<String> {
        let body = serde_json::json!({
            "parent": { "data_source_id": parent_data_source_id },
            "properties": properties,
        });
        let response: Value = self.api_json("POST", "v1/pages", Some(body)).await?;
        response
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| command_failed("create page response missing id"))
    }

    pub async fn update_page_properties(&self, page_id: &str, properties: Value) -> Result<()> {
        let body = serde_json::json!({ "properties": properties });
        let _: Value = self.api_json("PATCH", &format!("v1/pages/{page_id}"), Some(body)).await?;
        Ok(())
    }

    /// List pages in a data source (for picking a fixed relation target).
    pub async fn list_data_source_pages(&self, data_source_id: &str) -> Result<Vec<NotionPageRef>> {
        let mut results = Vec::new();
        let mut start_cursor: Option<String> = None;
        loop {
            let mut body = serde_json::json!({ "page_size": 100 });
            if let Some(cursor) = &start_cursor {
                body["start_cursor"] = Value::String(cursor.clone());
            }
            let response: Value = self
                .api_json("POST", &format!("v1/data_sources/{data_source_id}/query"), Some(body))
                .await?;
            let page_results =
                response.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
            for item in page_results {
                if let Some(page) = NotionPageRef::from_value(&item) {
                    results.push(page);
                }
            }
            let has_more = response.get("has_more").and_then(Value::as_bool).unwrap_or(false);
            if !has_more {
                break;
            }
            start_cursor = response.get("next_cursor").and_then(Value::as_str).map(str::to_string);
            if start_cursor.is_none() {
                break;
            }
        }
        results.sort_by(|a, b| a.title.cmp(&b.title));
        Ok(results)
    }

    pub async fn api_json<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let value = self.api_raw(method, path, body).await?;
        serde_json::from_value(value)
            .map_err(|source| PlatformError::InvalidJson { program: "ntn".into(), source }.into())
    }

    async fn api_raw(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let path = path.trim_start_matches('/');
        let mut cmd = Command::new("ntn");
        cmd.arg("api").arg(path).arg("-X").arg(method);
        if let Some(body) = body {
            let json = serde_json::to_string(&body)?;
            cmd.arg("--data").arg(json);
        }

        let output =
            cmd.output().await.map_err(|e| command_failed(format!("failed to spawn: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        if !output.status.success() {
            let msg = if stderr.is_empty() { stdout } else { stderr };
            return Err(command_failed(msg));
        }

        if stdout.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&stdout)
            .map_err(|source| PlatformError::InvalidJson { program: "ntn".into(), source }.into())
    }
}

#[derive(Debug, Clone)]
pub struct NotionDataSource {
    pub id: String,
    pub title: String,
}

impl NotionDataSource {
    fn from_value(value: &Value) -> Option<Self> {
        let object = value.get("object").and_then(Value::as_str).unwrap_or("data_source");
        if object != "data_source" && object != "database" {
            return None;
        }
        let id = value.get("id")?.as_str()?.to_string();
        let title = data_source_title(value).unwrap_or_else(|| id.clone());
        Some(Self { id, title })
    }
}

pub fn data_source_title(value: &Value) -> Option<String> {
    if let Some(name) = value.get("name").and_then(Value::as_str) {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let title = value.get("title")?.as_array()?;
    let mut parts = Vec::new();
    for item in title {
        if let Some(text) = item.pointer("/plain_text").and_then(Value::as_str) {
            parts.push(text.to_string());
        } else if let Some(text) = item.pointer("/text/content").and_then(Value::as_str) {
            parts.push(text.to_string());
        }
    }
    let joined = parts.join("").trim().to_string();
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

#[derive(Debug, Clone)]
pub struct NotionPageRef {
    pub id: String,
    pub title: String,
}

impl NotionPageRef {
    fn from_value(value: &Value) -> Option<Self> {
        let id = value.get("id")?.as_str()?.to_string();
        let title = page_title_from_properties(value).unwrap_or_else(|| id.clone());
        Some(Self { id, title })
    }
}

fn page_title_from_properties(page: &Value) -> Option<String> {
    let props = page.get("properties")?.as_object()?;
    for prop in props.values() {
        if prop.get("type").and_then(Value::as_str) != Some("title") {
            continue;
        }
        let title = prop.get("title")?.as_array()?;
        let mut parts = Vec::new();
        for item in title {
            if let Some(text) = item.pointer("/plain_text").and_then(Value::as_str) {
                parts.push(text.to_string());
            } else if let Some(text) = item.pointer("/text/content").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
        let joined = parts.join("").trim().to_string();
        if !joined.is_empty() {
            return Some(joined);
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct NotionDbProperty {
    pub id: String,
    pub name: String,
    pub field_type: String,
    pub options: Vec<String>,
    /// For `relation` properties: target data source id.
    pub relation_data_source_id: Option<String>,
}

pub fn parse_data_source_properties(data_source: &Value) -> Vec<NotionDbProperty> {
    let Some(props) = data_source.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, prop) in props {
        let id = prop.get("id").and_then(Value::as_str).unwrap_or(name).to_string();
        let field_type = prop.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        let options = property_options(prop, &field_type);
        let relation_data_source_id = if field_type == "relation" {
            prop.pointer("/relation/data_source_id").and_then(Value::as_str).map(str::to_string)
        } else {
            None
        };
        out.push(NotionDbProperty {
            id,
            name: name.clone(),
            field_type,
            options,
            relation_data_source_id,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn property_options(prop: &Value, field_type: &str) -> Vec<String> {
    let options = match field_type {
        "status" => prop.pointer("/status/options"),
        "select" => prop.pointer("/select/options"),
        _ => None,
    };
    let Some(options) = options.and_then(Value::as_array) else {
        return Vec::new();
    };
    options
        .iter()
        .filter_map(|opt| opt.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

pub fn run_ntn_interactive(args: &[&str]) -> Result<()> {
    use std::process::Stdio;
    let status = std::process::Command::new("ntn")
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| crate::error::Error::Config(format!("failed to spawn ntn: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(crate::error::Error::Config(format!("ntn {} failed", args.join(" "))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_data_source_from_search() {
        let value = json!({
            "object": "data_source",
            "id": "ds-1",
            "name": "Roadmap",
            "properties": {}
        });
        let ds = NotionDataSource::from_value(&value).unwrap();
        assert_eq!(ds.id, "ds-1");
        assert_eq!(ds.title, "Roadmap");
    }

    #[test]
    fn parses_title_rich_text() {
        let value = json!({
            "object": "data_source",
            "id": "ds-2",
            "title": [{ "plain_text": "Milestones" }]
        });
        assert_eq!(data_source_title(&value).as_deref(), Some("Milestones"));
    }
}
