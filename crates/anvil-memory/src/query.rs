//! Progressive Disclosure API: BM25 FTS5 `search`, `timeline`, `get`, and `session_context`.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::model::{MemoryKind, Observation, SearchHit, TimelineEntry};
use crate::store::sql::{OBS_COLUMNS, parse_freshness, row_to_observation};

const DEFAULT_SNIPPET_MAX_CHARS: usize = 140;

/// Sanitizes arbitrary user search input into safe quoted FTS5 terms.
pub fn sanitize_fts_query(raw: &str) -> String {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in raw.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(format!("\"{current}\""));
            current.clear();
        }
    }
    if !current.is_empty() {
        tokens.push(format!("\"{current}\""));
    }
    tokens.join(" ")
}

/// Performs FTS5 BM25 search returning compact `SearchHit` rows for progressive disclosure.
pub fn search(
    conn: &Connection,
    query: &str,
    kind: Option<MemoryKind>,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    let limit_i64 = i64::try_from(limit.max(1)).unwrap_or(50);
    let kind_filter = kind.map(MemoryKind::as_str);
    let fts_query = sanitize_fts_query(query);
    if fts_query.is_empty() {
        return recent_hits(conn, kind_filter, limit_i64);
    }

    let mut stmt = conn.prepare_cached(
        "SELECT o.id, o.topic_key, o.kind, o.title, o.updated_at, o.freshness,
                COALESCE(NULLIF(snippet(observations_fts, 2, '[', ']', '...', 16), ''), o.what)
         FROM observations_fts
         JOIN observations o ON o.rowid = observations_fts.rowid
         WHERE observations_fts MATCH ?1 AND (?2 IS NULL OR o.kind = ?2)
         ORDER BY bm25(observations_fts, 3.0, 2.0, 1.5, 1.0, 1.0, 1.5) ASC, o.updated_at DESC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![fts_query, kind_filter, limit_i64], row_to_search_hit)?;
    collect_rows(rows)
}

/// Returns chronological neighborhood entries centered around `id_or_topic`.
pub fn timeline(
    conn: &Connection,
    id_or_topic: &str,
    before: usize,
    after: usize,
) -> Result<Vec<TimelineEntry>> {
    let Some((anchor_id, anchor_ts)) = resolve_anchor(conn, id_or_topic)? else {
        return Ok(Vec::new());
    };
    let before_limit = i64::try_from(before).unwrap_or(10);
    let after_limit = i64::try_from(after).unwrap_or(10);

    let mut prev = fetch_neighbors(conn, &anchor_id, anchor_ts, before_limit, true)?;
    prev.reverse();
    let anchor = fetch_anchor(conn, &anchor_id)?;
    let next = fetch_neighbors(conn, &anchor_id, anchor_ts, after_limit, false)?;

    let mut result = prev;
    if let Some(entry) = anchor {
        result.push(entry);
    }
    result.extend(next);
    Ok(result)
}

/// Loads the complete `Observation` body by `id` on demand.
pub fn get(conn: &Connection, id: &str) -> Result<Option<Observation>> {
    let sql = format!("SELECT {OBS_COLUMNS} FROM observations WHERE id = ?1");
    let mut stmt = conn.prepare_cached(&sql)?;
    Ok(stmt.query_row([id], row_to_observation).optional()?)
}

/// Returns compact recent decisions, gotchas, domain rules, and active sessions within `max_tokens`.
pub fn session_context(conn: &Connection, max_tokens: usize, now_ms: i64) -> Result<Vec<SearchHit>> {
    if max_tokens == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT id, topic_key, kind, title, updated_at, freshness, what
         FROM observations
         WHERE expires_at IS NULL OR expires_at > ?1
         ORDER BY CASE kind WHEN 'decision' THEN 0 WHEN 'gotcha' THEN 1
                            WHEN 'domain' THEN 2 ELSE 3 END ASC, updated_at DESC
         LIMIT 64",
    )?;
    let rows = stmt.query_map([now_ms], row_to_search_hit)?;
    let mut out = Vec::new();
    let mut used_tokens = 0usize;
    for row in rows {
        let hit = row?;
        let est = estimate_hit_tokens(&hit);
        if !out.is_empty() && used_tokens + est > max_tokens {
            break;
        }
        used_tokens += est;
        out.push(hit);
        if used_tokens >= max_tokens {
            break;
        }
    }
    Ok(out)
}

fn recent_hits(conn: &Connection, kind_filter: Option<&str>, limit: i64) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, topic_key, kind, title, updated_at, freshness, what
         FROM observations WHERE (?1 IS NULL OR kind = ?1)
         ORDER BY updated_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![kind_filter, limit], row_to_search_hit)?;
    collect_rows(rows)
}

fn collect_rows<T, F>(rows: rusqlite::MappedRows<'_, F>) -> Result<Vec<T>>
where
    F: FnMut(&Row<'_>) -> rusqlite::Result<T>,
{
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn resolve_anchor(conn: &Connection, id_or_topic: &str) -> Result<Option<(String, i64)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, updated_at FROM observations WHERE id = ?1 OR topic_key = ?1 LIMIT 1",
    )?;
    Ok(stmt
        .query_row([id_or_topic], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?)
}

fn fetch_anchor(conn: &Connection, anchor_id: &str) -> Result<Option<TimelineEntry>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, topic_key, kind, title, created_at, updated_at, freshness
         FROM observations WHERE id = ?1",
    )?;
    Ok(stmt
        .query_row([anchor_id], |row| row_to_timeline_entry(row, true))
        .optional()?)
}

fn fetch_neighbors(
    conn: &Connection,
    anchor_id: &str,
    anchor_ts: i64,
    limit: i64,
    before: bool,
) -> Result<Vec<TimelineEntry>> {
    let sql = if before {
        "SELECT id, topic_key, kind, title, created_at, updated_at, freshness
         FROM observations
         WHERE (updated_at < ?1) OR (updated_at = ?1 AND id < ?2)
         ORDER BY updated_at DESC, id DESC LIMIT ?3"
    } else {
        "SELECT id, topic_key, kind, title, created_at, updated_at, freshness
         FROM observations
         WHERE (updated_at > ?1) OR (updated_at = ?1 AND id > ?2)
         ORDER BY updated_at ASC, id ASC LIMIT ?3"
    };
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(params![anchor_ts, anchor_id, limit], |row| {
        row_to_timeline_entry(row, false)
    })?;
    collect_rows(rows)
}

fn row_to_search_hit(row: &Row<'_>) -> rusqlite::Result<SearchHit> {
    let kind_raw: String = row.get(2)?;
    let freshness_raw: String = row.get(5)?;
    let raw_snippet: String = row.get(6)?;
    Ok(SearchHit {
        id: row.get(0)?,
        topic_key: row.get(1)?,
        kind: kind_raw.parse().unwrap_or(MemoryKind::Decision),
        title: row.get(3)?,
        updated_at: row.get(4)?,
        freshness: parse_freshness(&freshness_raw),
        snippet: truncate_chars(&raw_snippet, DEFAULT_SNIPPET_MAX_CHARS),
    })
}

fn row_to_timeline_entry(row: &Row<'_>, is_anchor: bool) -> rusqlite::Result<TimelineEntry> {
    let kind_raw: String = row.get(2)?;
    let freshness_raw: String = row.get(6)?;
    Ok(TimelineEntry {
        id: row.get(0)?,
        topic_key: row.get(1)?,
        kind: kind_raw.parse().unwrap_or(MemoryKind::Decision),
        title: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        freshness: parse_freshness(&freshness_raw),
        is_anchor,
    })
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut iter = text.chars();
    let taken: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{taken}...")
    } else {
        taken
    }
}

fn estimate_hit_tokens(hit: &SearchHit) -> usize {
    let chars = hit.title.len() + hit.topic_key.len() + hit.snippet.len() + 32;
    chars.div_ceil(4)
}
