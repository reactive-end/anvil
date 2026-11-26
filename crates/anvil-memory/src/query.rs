//! Progressive Disclosure API: BM25 FTS5 `search`, `timeline`, `get`, and `session_context`.

mod map;

use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{MemoryKind, Observation, SearchHit, TimelineEntry};
use crate::store::sql::{OBS_COLUMNS, row_to_observation};
use map::{collect_rows, estimate_hit_tokens, row_to_search_hit, row_to_timeline_entry};

/// Words that carry no meaning in a memory query; dropped so they do not decide the ranking.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "can", "do", "does", "for", "how", "i", "in", "is", "it", "my", "of",
    "on", "or", "should", "the", "to", "what", "when", "where", "which", "who", "why", "with",
    "como", "cual", "de", "del", "el", "en", "es", "la", "las", "lo", "los", "para", "por", "que",
    "se", "un", "una", "y",
];

/// Turns arbitrary search input into quoted FTS5 terms joined by `OR`, so a record that matches
/// only some words is still found; BM25 ranks records that match more and rarer terms first.
pub fn sanitize_fts_query(raw: &str) -> String {
    let mut tokens: Vec<String> = Vec::new();
    for word in raw.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')) {
        let lower = word.to_lowercase();
        if word.is_empty() || STOPWORDS.contains(&lower.as_str()) {
            continue;
        }
        let quoted = format!("\"{word}\"");
        if !tokens.contains(&quoted) {
            tokens.push(quoted);
        }
    }
    if tokens.is_empty() {
        // Only stopwords: search them anyway rather than returning unrelated recent records.
        tokens = raw
            .split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
            .filter(|word| !word.is_empty())
            .map(|word| format!("\"{word}\""))
            .collect();
    }
    tokens.join(" OR ")
}

/// Performs FTS5 BM25 search returning compact `SearchHit` rows for progressive disclosure.
pub fn search(
    conn: &Connection,
    query: &str,
    kind: Option<MemoryKind>,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    let now_ms = Utc::now().timestamp_millis();
    let limit_i64 = i64::try_from(limit.max(1)).unwrap_or(50);
    let kind_filter = kind.map(MemoryKind::as_str);
    let fts_query = sanitize_fts_query(query);
    if fts_query.is_empty() {
        return recent_hits(conn, kind_filter, now_ms, limit_i64);
    }

    let mut stmt = conn.prepare_cached(
        "SELECT o.id, o.topic_key, o.kind, o.title, o.updated_at, o.freshness,
                COALESCE(NULLIF(snippet(observations_fts, 2, '[', ']', '...', 16), ''), o.what)
         FROM observations_fts
         JOIN observations o ON o.rowid = observations_fts.rowid
         WHERE observations_fts MATCH ?1
           AND (?2 IS NULL OR o.kind = ?2)
           AND (o.expires_at IS NULL OR o.expires_at > ?3)
         ORDER BY bm25(observations_fts, 3.0, 2.0, 1.5, 1.0, 1.0, 1.5) ASC, o.updated_at DESC
         LIMIT ?4",
    )?;
    let rows =
        stmt.query_map(params![fts_query, kind_filter, now_ms, limit_i64], row_to_search_hit)?;
    collect_rows(rows)
}

/// Returns chronological neighborhood entries centered around `id_or_topic`.
pub fn timeline(
    conn: &Connection,
    id_or_topic: &str,
    before: usize,
    after: usize,
) -> Result<Vec<TimelineEntry>> {
    let now_ms = Utc::now().timestamp_millis();
    let Some((anchor_id, anchor_ts)) = resolve_anchor(conn, id_or_topic, now_ms)? else {
        return Ok(Vec::new());
    };
    let before_limit = i64::try_from(before).unwrap_or(10);
    let after_limit = i64::try_from(after).unwrap_or(10);

    let mut prev = fetch_neighbors(conn, &anchor_id, anchor_ts, now_ms, before_limit, true)?;
    prev.reverse();
    let anchor = fetch_anchor(conn, &anchor_id, now_ms)?;
    let next = fetch_neighbors(conn, &anchor_id, anchor_ts, now_ms, after_limit, false)?;

    let mut result = prev;
    if let Some(entry) = anchor {
        result.push(entry);
    }
    result.extend(next);
    Ok(result)
}

/// Loads the complete non-expired `Observation` body by `id` on demand.
pub fn get(conn: &Connection, id: &str) -> Result<Option<Observation>> {
    let now_ms = Utc::now().timestamp_millis();
    let sql = format!(
        "SELECT {OBS_COLUMNS} FROM observations
         WHERE id = ?1 AND (expires_at IS NULL OR expires_at > ?2)"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    Ok(stmt.query_row(params![id, now_ms], row_to_observation).optional()?)
}

/// Kinds in the order a fresh agent needs them: working rules and project knowledge, decisions,
/// pitfalls, then sessions.
const CONTEXT_KIND_ORDER: &[MemoryKind] =
    &[MemoryKind::Domain, MemoryKind::Decision, MemoryKind::Gotcha, MemoryKind::Session];

/// Returns compact recent memory within `max_tokens`, taking the newest record of each kind in
/// turn so rules, decisions, and pitfalls all appear instead of one kind filling the budget.
pub fn session_context(
    conn: &Connection,
    max_tokens: usize,
    now_ms: i64,
) -> Result<Vec<SearchHit>> {
    if max_tokens == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT id, topic_key, kind, title, updated_at, freshness, what
         FROM observations
         WHERE expires_at IS NULL OR expires_at > ?1
         ORDER BY updated_at DESC
         LIMIT 128",
    )?;
    let rows = collect_rows(stmt.query_map([now_ms], row_to_search_hit)?)?;
    let mut queues: Vec<std::collections::VecDeque<SearchHit>> =
        CONTEXT_KIND_ORDER.iter().map(|_| std::collections::VecDeque::new()).collect();
    for hit in rows {
        let slot = CONTEXT_KIND_ORDER.iter().position(|k| *k == hit.kind).unwrap_or(0);
        queues[slot].push_back(hit);
    }
    let mut out = Vec::new();
    let mut used_tokens = 0usize;
    while queues.iter().any(|q| !q.is_empty()) {
        for queue in &mut queues {
            let Some(hit) = queue.pop_front() else {
                continue;
            };
            let est = estimate_hit_tokens(&hit);
            if !out.is_empty() && used_tokens + est > max_tokens {
                return Ok(out);
            }
            used_tokens += est;
            out.push(hit);
        }
    }
    Ok(out)
}

fn recent_hits(
    conn: &Connection,
    kind_filter: Option<&str>,
    now_ms: i64,
    limit: i64,
) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, topic_key, kind, title, updated_at, freshness, what
         FROM observations
         WHERE (?1 IS NULL OR kind = ?1)
           AND (expires_at IS NULL OR expires_at > ?2)
         ORDER BY updated_at DESC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![kind_filter, now_ms, limit], row_to_search_hit)?;
    collect_rows(rows)
}

fn resolve_anchor(
    conn: &Connection,
    id_or_topic: &str,
    now_ms: i64,
) -> Result<Option<(String, i64)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, updated_at FROM observations
         WHERE (id = ?1 OR topic_key = ?1)
           AND (expires_at IS NULL OR expires_at > ?2)
         LIMIT 1",
    )?;
    Ok(stmt
        .query_row(params![id_or_topic, now_ms], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?)
}

fn fetch_anchor(conn: &Connection, anchor_id: &str, now_ms: i64) -> Result<Option<TimelineEntry>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, topic_key, kind, title, created_at, updated_at, freshness
         FROM observations
         WHERE id = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
    )?;
    Ok(stmt
        .query_row(params![anchor_id, now_ms], |row| row_to_timeline_entry(row, true))
        .optional()?)
}

fn fetch_neighbors(
    conn: &Connection,
    anchor_id: &str,
    anchor_ts: i64,
    now_ms: i64,
    limit: i64,
    before: bool,
) -> Result<Vec<TimelineEntry>> {
    let sql = if before {
        "SELECT id, topic_key, kind, title, created_at, updated_at, freshness
         FROM observations
         WHERE ((updated_at < ?1) OR (updated_at = ?1 AND id < ?2))
           AND (expires_at IS NULL OR expires_at > ?3)
         ORDER BY updated_at DESC, id DESC LIMIT ?4"
    } else {
        "SELECT id, topic_key, kind, title, created_at, updated_at, freshness
         FROM observations
         WHERE ((updated_at > ?1) OR (updated_at = ?1 AND id > ?2))
           AND (expires_at IS NULL OR expires_at > ?3)
         ORDER BY updated_at ASC, id ASC LIMIT ?4"
    };
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(params![anchor_ts, anchor_id, now_ms, limit], |row| {
        row_to_timeline_entry(row, false)
    })?;
    collect_rows(rows)
}
