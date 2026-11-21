//! Data types for governed ratchet candidates, evaluations, and promoted rules.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::traits::Severity;

/// Input payload when logging a new recurring agent mistake for ratchet review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateInput {
    pub rule_name: String,
    pub description: String,
    pub pattern: String,
    pub languages: Vec<String>,
    pub high_impact: bool,
    pub deterministic: bool,
    pub estimated_runtime_ms: u64,
    pub example: Option<String>,
}

/// Candidate rule stored in `.harness/candidates/<id>.json` before promotion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatchetCandidate {
    pub id: String,
    pub rule_name: String,
    pub description: String,
    #[serde(default)]
    pub pattern: Option<String>,
    pub languages: Vec<String>,
    #[serde(default)]
    pub file_globs: Vec<String>,
    #[serde(default)]
    pub severity: Option<Severity>,
    pub recurrence: u32,
    pub high_impact: bool,
    pub deterministic: bool,
    pub estimated_runtime_ms: u64,
    pub fp_checked: bool,
    pub false_positives_found: usize,
    pub examples: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Alias for [`RatchetCandidate`].
pub type RuleCandidate = RatchetCandidate;

/// Result of evaluating a candidate against the 3 mandatory ratchet filters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatchetEvaluation {
    pub id: String,
    pub eligible: bool,
    pub filter_recurrence_or_impact: bool,
    pub filter_deterministic_fast: bool,
    pub filter_zero_false_positives: bool,
    pub measured_runtime_ms: u64,
    pub reasons: Vec<String>,
}

/// Graduated deterministic rule stored in `.harness/rules/<id>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotedRule {
    pub id: String,
    pub rule_name: String,
    pub description: String,
    #[serde(default)]
    pub pattern: Option<String>,
    pub languages: Vec<String>,
    #[serde(default)]
    pub file_globs: Vec<String>,
    #[serde(default)]
    pub severity: Option<Severity>,
    pub recurrence_at_promotion: u32,
    pub promoted_at: DateTime<Utc>,
}
