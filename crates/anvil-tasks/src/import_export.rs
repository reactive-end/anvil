//! Import and export of external `feature_list.json` requirement specifications into
//! Anvil's conflict-free `.harness/tasks/<ID>/` ledger.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::{Value, json};

use anvil_core::fs_util::sanitize_author;

use crate::builder::{build_root_tasks_json, consolidate_repo_tasks};
use crate::identity::resolve_author;
use crate::ledger::{append_entry_with_evidence, task_dir};
use crate::model::{TaskMeta, TaskStatus};
use crate::{atomic_write_json, read_json_file};

/// Parsed external JSON feature/task specification file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedFeatureList {
    pub project: Option<String>,
    pub updated: Option<String>,
    pub features: Vec<ImportedFeature>,
}

/// Single requirement item normalized from `feature_list.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedFeature {
    pub id: String,
    pub name: String,
    pub behavior: Option<String>,
    pub verify: Option<String>,
    pub status: TaskStatus,
    pub evidence: Option<String>,
}

/// Summary returned after decomposing a feature list into `.harness/tasks/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub project: Option<String>,
    pub imported: usize,
    pub updated: usize,
    pub skipped: usize,
    pub total_tasks: usize,
}

/// Parses `{ "project", "updated", "features": [...] }`, `{ "tasks": [...] }`, or `[...]`.
pub fn parse_feature_list_json(raw: &str) -> Result<ImportedFeatureList> {
    let val: Value = serde_json::from_str(raw).context("invalid JSON in feature list file")?;
    let (project, updated, items) = extract_items(&val)?;
    if items.is_empty() {
        bail!("feature list JSON contains 0 features/tasks");
    }

    let mut features = Vec::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        features.push(parse_single_feature(item, idx)?);
    }
    Ok(ImportedFeatureList {
        project,
        updated,
        features,
    })
}

fn extract_items(val: &Value) -> Result<(Option<String>, Option<String>, &[Value])> {
    if let Some(arr) = val.as_array() {
        return Ok((None, None, arr.as_slice()));
    }
    let obj = val
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("expected top-level JSON object or array"))?;
    let project = opt_trimmed_str(obj.get("project"));
    let updated = opt_trimmed_str(obj.get("updated"));
    let arr = obj
        .get("features")
        .or_else(|| obj.get("tasks"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("expected `features` (or `tasks`) JSON array"))?;
    Ok((project, updated, arr.as_slice()))
}

fn parse_single_feature(item: &Value, idx: usize) -> Result<ImportedFeature> {
    let obj = item
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("feature at index {idx} must be a JSON object"))?;
    let raw_id = opt_trimmed_str(obj.get("id").or_else(|| obj.get("task_id")))
        .unwrap_or_else(|| format!("F{:03}", idx + 1));
    let id = sanitize_task_id(&raw_id, idx);
    let name = opt_trimmed_str(
        obj.get("name")
            .or_else(|| obj.get("title"))
            .or_else(|| obj.get("request")),
    )
    .ok_or_else(|| anyhow::anyhow!("feature `{id}` is missing required `name`/`title`"))?;
    let behavior = opt_trimmed_str(obj.get("behavior").or_else(|| obj.get("description")));
    let verify = opt_trimmed_str(obj.get("verify"));
    let evidence = opt_trimmed_str(obj.get("evidence"));
    let status = parse_feature_status(obj.get("status"), obj.get("passes"));

    Ok(ImportedFeature {
        id,
        name,
        behavior,
        verify,
        status,
        evidence,
    })
}

fn parse_feature_status(status_val: Option<&Value>, passes_val: Option<&Value>) -> TaskStatus {
    if let Some(raw) = status_val.and_then(Value::as_str) {
        match raw.trim().to_ascii_lowercase().as_str() {
            "done" | "completed" | "passed" | "passing" | "verified" => return TaskStatus::Done,
            "in_progress" | "in-progress" | "wip" | "partial" | "progress" => {
                return TaskStatus::InProgress;
            }
            "blocked" | "failing" | "failed" => return TaskStatus::Blocked,
            "not_started" | "not-started" | "pending" | "todo" | "open" => {
                return TaskStatus::NotStarted;
            }
            _ => {}
        }
    }
    match passes_val.and_then(Value::as_bool) {
        Some(true) => TaskStatus::Done,
        _ => TaskStatus::NotStarted,
    }
}

/// Imports every feature into an isolated `.harness/tasks/<ID>/` folder without overwriting
/// existing team entries (unless `force` is `true`), then rebuilds root `tasks.json`.
pub fn import_feature_list(
    repo: &Path,
    list: &ImportedFeatureList,
    author: Option<&str>,
    force: bool,
) -> Result<ImportReport> {
    let resolved_author = match author {
        Some(a) => sanitize_author(a),
        None => resolve_author(repo)?,
    };
    let mut imported = 0usize;
    let mut updated = 0usize;
    let mut skipped = 0usize;

    for feat in &list.features {
        match import_one_feature(repo, list.project.as_deref(), feat, &resolved_author, force)? {
            ImportAction::Imported => imported += 1,
            ImportAction::Updated => updated += 1,
            ImportAction::Skipped => skipped += 1,
        }
    }

    let consolidated = build_root_tasks_json(repo)?;
    Ok(ImportReport {
        project: list.project.clone(),
        imported,
        updated,
        skipped,
        total_tasks: consolidated.task_count,
    })
}

enum ImportAction {
    Imported,
    Updated,
    Skipped,
}

fn import_one_feature(
    repo: &Path,
    project: Option<&str>,
    feat: &ImportedFeature,
    author: &str,
    force: bool,
) -> Result<ImportAction> {
    let dir = task_dir(repo, &feat.id);
    let meta_path = dir.join("meta.json");
    if !meta_path.exists() {
        fs::create_dir_all(dir.join("entries"))
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let meta = TaskMeta {
            task_id: feat.id.clone(),
            created_by: author.to_owned(),
            request: feat.name.clone(),
            project: project.map(str::to_owned),
            behavior: feat.behavior.clone(),
            verify: feat.verify.clone(),
            created_at: Utc::now(),
        };
        atomic_write_json(&meta_path, &meta)?;
        let summary = feat
            .evidence
            .clone()
            .unwrap_or_else(|| "Imported from feature list".to_owned());
        append_entry_with_evidence(
            repo,
            &feat.id,
            author,
            feat.status,
            &summary,
            feat.evidence.as_deref(),
            &[],
        )?;
        return Ok(ImportAction::Imported);
    }

    update_existing_feature(repo, &meta_path, project, feat, author, force)
}

fn update_existing_feature(
    repo: &Path,
    meta_path: &Path,
    project: Option<&str>,
    feat: &ImportedFeature,
    author: &str,
    force: bool,
) -> Result<ImportAction> {
    let mut existing: TaskMeta = read_json_file(meta_path)?;
    let new_project = project.map(str::to_owned).or_else(|| existing.project.clone());
    let new_behavior = feat.behavior.clone().or_else(|| existing.behavior.clone());
    let new_verify = feat.verify.clone().or_else(|| existing.verify.clone());
    let meta_changed = existing.request != feat.name
        || existing.project != new_project
        || existing.behavior != new_behavior
        || existing.verify != new_verify;
    if meta_changed {
        existing.request = feat.name.clone();
        existing.project = new_project;
        existing.behavior = new_behavior;
        existing.verify = new_verify;
        atomic_write_json(meta_path, &existing)?;
    }
    if force {
        let summary = feat
            .evidence
            .clone()
            .unwrap_or_else(|| format!("Re-imported status ({})", feat.status));
        append_entry_with_evidence(
            repo,
            &feat.id,
            author,
            feat.status,
            &summary,
            feat.evidence.as_deref(),
            &[],
        )?;
        return Ok(ImportAction::Updated);
    }
    if meta_changed {
        Ok(ImportAction::Updated)
    } else {
        Ok(ImportAction::Skipped)
    }
}

/// Exports consolidated `.harness/tasks/` back to the single-file `feature_list.json` format.
pub fn export_feature_list(repo: &Path, project_filter: Option<&str>) -> Result<Value> {
    let consolidated = consolidate_repo_tasks(repo)?;
    let inferred_project = project_filter.map(str::to_owned).or_else(|| {
        consolidated
            .tasks
            .iter()
            .find_map(|t| t.project.clone())
    });

    let features: Vec<Value> = consolidated
        .tasks
        .into_iter()
        .filter(|t| {
            project_filter.is_none_or(|p| t.project.as_deref() == Some(p))
        })
        .map(|t| {
            let evidence = t.evidence.unwrap_or_else(|| t.summary.clone());
            json!({
                "id": t.task_id,
                "name": t.request,
                "behavior": t.behavior.unwrap_or_default(),
                "verify": t.verify,
                "status": t.status.to_string(),
                "evidence": evidence,
            })
        })
        .collect();

    Ok(json!({
        "project": inferred_project.unwrap_or_else(|| "anvil".to_owned()),
        "updated": Utc::now().format("%Y-%m-%d").to_string(),
        "features": features,
    }))
}

fn opt_trimmed_str(val: Option<&Value>) -> Option<String> {
    val.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn sanitize_task_id(raw: &str, idx: usize) -> String {
    let clean: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = clean.trim_matches('-');
    if trimmed.is_empty() {
        format!("F{:03}", idx + 1)
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::builder::read_single_task;

    #[test]
    fn parses_voz_schema_imports_conflict_free_and_exports_roundtrip() {
        let dir = TempDir::new().unwrap();
        let raw = r#"{
            "project": "voz",
            "updated": "2026-11-24",
            "features": [
                {
                    "id": "F001",
                    "name": "VOZ-RF-01 Crear el bucket de artefactos",
                    "behavior": "Crear el bucket de artefactos",
                    "verify": "echo ok",
                    "status": "not_started",
                    "evidence": "En codigo"
                },
                {
                    "id": "F003",
                    "name": "VOZ-RF-03 VAD 512",
                    "behavior": "Detectar inicio y fin de voz",
                    "verify": null,
                    "status": "not_started",
                    "evidence": "Pendiente"
                }
            ]
        }"#;
        let parsed = parse_feature_list_json(raw).unwrap();
        assert_eq!(parsed.project.as_deref(), Some("voz"));
        assert_eq!(parsed.features.len(), 2);

        let r1 = import_feature_list(dir.path(), &parsed, Some("alice"), false).unwrap();
        assert_eq!(r1.imported, 2);
        assert_eq!(r1.total_tasks, 2);
        let f1 = read_single_task(dir.path(), "F001").unwrap();
        assert_eq!(f1.status, TaskStatus::NotStarted);
        assert_eq!(f1.verify.as_deref(), Some("echo ok"));

        // Progress on F001 by bob must survive an idempotent re-import without `--force`.
        append_entry_with_evidence(
            dir.path(),
            "F001",
            "bob",
            TaskStatus::Done,
            "Verified bucket",
            Some("Verified exit=0"),
            &[],
        )
        .unwrap();
        let r2 = import_feature_list(dir.path(), &parsed, Some("alice"), false).unwrap();
        assert_eq!(r2.imported, 0);
        assert_eq!(r2.skipped, 2);
        let f1_after = read_single_task(dir.path(), "F001").unwrap();
        assert_eq!(f1_after.status, TaskStatus::Done);
        assert_eq!(f1_after.evidence.as_deref(), Some("Verified exit=0"));

        let exported = export_feature_list(dir.path(), Some("voz")).unwrap();
        assert_eq!(exported["project"], "voz");
        assert_eq!(exported["features"].as_array().unwrap().len(), 2);
        assert_eq!(exported["features"][0]["status"], "done");
        assert!(exported["features"][1]["verify"].is_null());
    }
}
