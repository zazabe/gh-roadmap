use std::collections::{BTreeMap, HashMap};

use crate::model::{FieldSource, ProjectFieldMapping};
use crate::platform::{IssueFieldDefinition, ProjectField, ProjectMeta};
use crate::prefetch::ProjectItemState;

/// Where a local frontmatter field should be written on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldTarget {
    /// GitHub Issue Fields (`IFD_*` / `setIssueFieldValue`).
    Issue { field_name: String },
    /// GitHub Projects V2 fields (`PVTF_*` / `gh project item-edit`).
    Project { field_id: String, field_name: String, field_type: String },
}

/// Resolve each local field key to an issue-field or project-field target.
///
/// Defaults: if an issue-field definition exists for the key, use it (Start/Target
/// date). Otherwise use a writable Project V2 mapping (Estimate). Explicit
/// `source` on the sync-map mapping overrides that. `status` is owned by
/// [`crate::plan::ActionKind::SetProjectStatus`].
pub fn resolve_field_target(
    local_key: &str,
    mappings: &HashMap<String, ProjectFieldMapping>,
    issue_defs: Option<&[IssueFieldDefinition]>,
    project_meta: Option<&ProjectMeta>,
) -> Option<FieldTarget> {
    if local_key == "status" {
        return None;
    }

    let mapping = mappings.get(local_key);
    let issue_target = find_issue_target(local_key, mapping, issue_defs);
    let project_target =
        mapping.and_then(|mapping| project_target_for_mapping(mapping, project_meta));

    match mapping.and_then(ProjectFieldMapping::effective_source) {
        // Issue-field definitions win when present (linked Start/Target dates),
        // even if pull stamped `source: project` because GraphQL reported DATE.
        Some(FieldSource::Issue) => issue_target,
        Some(FieldSource::Project) | None => issue_target.or(project_target),
    }
}

fn project_target_for_mapping(
    mapping: &ProjectFieldMapping,
    project_meta: Option<&ProjectMeta>,
) -> Option<FieldTarget> {
    let meta_field = project_meta.and_then(|meta| {
        field_by_id(meta, &mapping.field_id)
            .or_else(|| meta.fields.iter().find(|field| field.name == mapping.field_name))
    });

    // Prefer live project metadata type; fall back to sync-map type when gh
    // returns an empty dataType (common for some field-list entries).
    let field_type = meta_field
        .map(|field| field.field_type.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(mapping.field_type.as_str());
    if !is_writable_project_type(field_type) {
        return None;
    }

    let field_id = meta_field.map_or(mapping.field_id.as_str(), |field| field.id.as_str());
    let field_name = meta_field.map_or(mapping.field_name.as_str(), |field| field.name.as_str());

    Some(FieldTarget::Project {
        field_id: field_id.to_string(),
        field_name: field_name.to_string(),
        field_type: field_type.to_string(),
    })
}

pub fn fields_need_sync(
    local_fields: &BTreeMap<String, Option<String>>,
    mappings: &HashMap<String, ProjectFieldMapping>,
    remote_issue_fields: Option<&HashMap<String, String>>,
    issue_field_defs: Option<&[IssueFieldDefinition]>,
    project_item: Option<&ProjectItemState>,
    project_meta: Option<&ProjectMeta>,
) -> bool {
    local_fields.iter().any(|(local_key, local_value)| {
        let Some(target) =
            resolve_field_target(local_key, mappings, issue_field_defs, project_meta)
        else {
            return false;
        };
        let local_norm = normalize_field_value(local_value.as_deref());
        let remote_norm = match &target {
            FieldTarget::Issue { field_name } => remote_issue_fields
                .and_then(|fields| fields.get(field_name))
                .and_then(|value| normalize_field_value(Some(value.as_str()))),
            FieldTarget::Project { field_id, .. } => project_item
                .and_then(|item| item.field_values.get(field_id))
                .and_then(|value| normalize_field_value(Some(value.as_str()))),
        };
        !field_values_equal(&target, local_norm, remote_norm)
    })
}

pub fn field_values_equal(target: &FieldTarget, local: Option<&str>, remote: Option<&str>) -> bool {
    match target {
        FieldTarget::Project { field_type, .. } if field_type.eq_ignore_ascii_case("NUMBER") => {
            match (local, remote) {
                (None, None) => true,
                (Some(l), Some(r)) => match (l.parse::<f64>(), r.parse::<f64>()) {
                    (Ok(a), Ok(b)) => (a - b).abs() < f64::EPSILON,
                    _ => l == r,
                },
                _ => false,
            }
        }
        _ => local == remote,
    }
}

pub fn normalize_field_value(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

pub fn is_writable_project_type(field_type: &str) -> bool {
    matches!(field_type.to_ascii_uppercase().as_str(), "NUMBER" | "TEXT" | "DATE" | "SINGLE_SELECT")
}

/// Built-in / read-only project columns that should not be sync-mapped.
pub fn is_builtin_project_type(field_type: &str) -> bool {
    matches!(
        field_type.to_ascii_uppercase().as_str(),
        "ASSIGNEES"
            | "LABELS"
            | "LINKED_PULL_REQUESTS"
            | "MILESTONE"
            | "REPOSITORY"
            | "REVIEWERS"
            | "PARENT_ISSUE"
            | "SUB_ISSUES_PROGRESS"
            | "TITLE"
            | "TRACKS"
            | "TRACKED_BY"
            | "CREATED_AT"
            | "UPDATED_AT"
            | "CLOSED_AT"
    )
}

/// Built-in project field names that are never local sync targets.
pub fn is_builtin_project_field_name(name: &str) -> bool {
    matches!(
        field_key(name).as_str(),
        "assignees"
            | "labels"
            | "linked_pull_requests"
            | "milestone"
            | "repository"
            | "reviewers"
            | "parent_issue"
            | "sub_issues_progress"
            | "title"
            | "tracks"
            | "tracked_by"
            | "created"
            | "updated"
            | "closed"
            | "status" // owned by SetProjectStatus
    )
}

pub fn should_sync_map_project_field(name: &str, field_type: &str) -> bool {
    if is_builtin_project_field_name(name) || is_builtin_project_type(field_type) {
        return false;
    }
    // Keep writable project fields and unknown/empty types (linked issue dates).
    is_writable_project_type(field_type) || field_type.is_empty()
}

pub fn field_key(name: &str) -> String {
    let mut key = String::new();
    let mut last_underscore = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            key.push(ch.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore {
            key.push('_');
            last_underscore = true;
        }
    }
    key.trim_matches('_').to_string()
}

fn find_issue_target(
    local_key: &str,
    mapping: Option<&ProjectFieldMapping>,
    issue_defs: Option<&[IssueFieldDefinition]>,
) -> Option<FieldTarget> {
    let defs = issue_defs?;
    if let Some(mapping) = mapping {
        if defs.iter().any(|field| field.name == mapping.field_name) {
            return Some(FieldTarget::Issue { field_name: mapping.field_name.clone() });
        }
    }
    defs.iter()
        .find(|field| field_key(&field.name) == local_key)
        .map(|field| FieldTarget::Issue { field_name: field.name.clone() })
}

fn field_by_id<'a>(meta: &'a ProjectMeta, field_id: &str) -> Option<&'a ProjectField> {
    meta.fields.iter().find(|field| field.id == field_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{IssueFieldDefinition, ProjectField, ProjectFieldOption, ProjectMeta};
    use crate::prefetch::ProjectItemState;

    fn project_meta() -> ProjectMeta {
        ProjectMeta {
            project_id: "P_1".into(),
            fields: vec![
                ProjectField {
                    id: "PVTF_estimate".into(),
                    name: "Estimate".into(),
                    field_type: "NUMBER".into(),
                    options: vec![],
                },
                ProjectField {
                    id: "PVTF_start".into(),
                    name: "Start date".into(),
                    // Linked issue fields often show up in field-list with empty type.
                    field_type: String::new(),
                    options: vec![],
                },
                ProjectField {
                    id: "PVTF_status".into(),
                    name: "Status".into(),
                    field_type: "SINGLE_SELECT".into(),
                    options: vec![ProjectFieldOption {
                        id: "opt_1".into(),
                        name: "Backlog".into(),
                    }],
                },
            ],
        }
    }

    fn issue_defs() -> Vec<IssueFieldDefinition> {
        vec![
            IssueFieldDefinition {
                id: "IFD_start".into(),
                name: "Start date".into(),
                data_type: "DATE".into(),
                options: vec![],
            },
            IssueFieldDefinition {
                id: "IFD_target".into(),
                name: "Target date".into(),
                data_type: "DATE".into(),
                options: vec![],
            },
        ]
    }

    fn mappings() -> HashMap<String, ProjectFieldMapping> {
        HashMap::from([
            (
                "estimate".into(),
                ProjectFieldMapping {
                    field_id: "PVTF_estimate".into(),
                    field_name: "Estimate".into(),
                    field_type: "NUMBER".into(),
                    source: Some(FieldSource::Project),
                },
            ),
            (
                "start_date".into(),
                ProjectFieldMapping {
                    field_id: "PVTF_start".into(),
                    field_name: "Start date".into(),
                    field_type: String::new(),
                    source: Some(FieldSource::Issue),
                },
            ),
        ])
    }

    #[test]
    fn routes_estimate_to_project_and_dates_to_issue_fields() {
        let meta = project_meta();
        let defs = issue_defs();
        let map = mappings();

        assert_eq!(
            resolve_field_target("estimate", &map, Some(&defs), Some(&meta)),
            Some(FieldTarget::Project {
                field_id: "PVTF_estimate".into(),
                field_name: "Estimate".into(),
                field_type: "NUMBER".into(),
            })
        );
        assert_eq!(
            resolve_field_target("start_date", &map, Some(&defs), Some(&meta)),
            Some(FieldTarget::Issue { field_name: "Start date".into() })
        );
        assert_eq!(resolve_field_target("status", &map, Some(&defs), Some(&meta)), None);
    }

    #[test]
    fn type_project_shorthand_selects_project_source() {
        let meta = project_meta();
        let defs = issue_defs();
        let map = HashMap::from([(
            "estimate".into(),
            ProjectFieldMapping {
                field_id: "PVTF_estimate".into(),
                field_name: "Estimate".into(),
                field_type: "project".into(),
                source: None,
            },
        )]);

        assert_eq!(
            resolve_field_target("estimate", &map, Some(&defs), Some(&meta)),
            Some(FieldTarget::Project {
                field_id: "PVTF_estimate".into(),
                field_name: "Estimate".into(),
                field_type: "NUMBER".into(),
            })
        );
    }

    #[test]
    fn prefers_issue_field_when_project_entry_has_empty_type() {
        let meta = project_meta();
        let defs = issue_defs();
        // No explicit source — mirrors an older sync-map that listed linked dates.
        let map = HashMap::from([(
            "start_date".into(),
            ProjectFieldMapping {
                field_id: "PVTF_start".into(),
                field_name: "Start date".into(),
                field_type: String::new(),
                source: None,
            },
        )]);

        assert_eq!(
            resolve_field_target("start_date", &map, Some(&defs), Some(&meta)),
            Some(FieldTarget::Issue { field_name: "Start date".into() })
        );
    }

    #[test]
    fn project_source_uses_mapping_type_when_meta_type_empty() {
        let meta = ProjectMeta {
            project_id: "P_1".into(),
            fields: vec![ProjectField {
                id: "PVTF_estimate".into(),
                name: "Estimate".into(),
                field_type: String::new(),
                options: vec![],
            }],
        };
        let map = HashMap::from([(
            "estimate".into(),
            ProjectFieldMapping {
                field_id: "PVTF_estimate".into(),
                field_name: "Estimate".into(),
                field_type: "NUMBER".into(),
                source: Some(FieldSource::Project),
            },
        )]);

        assert_eq!(
            resolve_field_target("estimate", &map, Some(&[]), Some(&meta)),
            Some(FieldTarget::Project {
                field_id: "PVTF_estimate".into(),
                field_name: "Estimate".into(),
                field_type: "NUMBER".into(),
            })
        );
    }

    #[test]
    fn needs_sync_when_project_estimate_missing() {
        let meta = project_meta();
        let defs = issue_defs();
        let map = mappings();
        let local = BTreeMap::from([
            ("estimate".into(), Some("5".into())),
            ("start_date".into(), Some("2026-07-01".into())),
        ]);
        let issue_fields = HashMap::from([("Start date".into(), "2026-07-01".into())]);
        let item = ProjectItemState { item_id: "PVTI_1".into(), field_values: HashMap::new() };

        assert!(fields_need_sync(
            &local,
            &map,
            Some(&issue_fields),
            Some(defs.as_slice()),
            Some(&item),
            Some(&meta),
        ));
    }

    #[test]
    fn no_sync_when_estimate_and_dates_match() {
        let meta = project_meta();
        let defs = issue_defs();
        let map = mappings();
        let local = BTreeMap::from([
            ("estimate".into(), Some("5".into())),
            ("start_date".into(), Some("2026-07-01".into())),
        ]);
        let issue_fields = HashMap::from([("Start date".into(), "2026-07-01".into())]);
        let item = ProjectItemState {
            item_id: "PVTI_1".into(),
            field_values: HashMap::from([("PVTF_estimate".into(), "5".into())]),
        };

        assert!(!fields_need_sync(
            &local,
            &map,
            Some(&issue_fields),
            Some(defs.as_slice()),
            Some(&item),
            Some(&meta),
        ));
    }

    #[test]
    fn number_equality_tolerates_float_formatting() {
        let target = FieldTarget::Project {
            field_id: "PVTF_estimate".into(),
            field_name: "Estimate".into(),
            field_type: "NUMBER".into(),
        };
        assert!(field_values_equal(&target, Some("5"), Some("5.0")));
        assert!(!field_values_equal(&target, Some("5"), Some("8")));
    }
}
