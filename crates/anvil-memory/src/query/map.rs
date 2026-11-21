//! Row mapping and progressive-disclosure formatting helpers for memory queries.

use anyhow::Result;
use rusqlite::Row;

use crate::model::{MemoryKind, SearchHit, TimelineEntry};
use crate::store::sql::parse_freshness;

const DEFAULT_SNIPPET_MAX_CHARS: usize = 140;

pub(crate) fn collect_rows<T, F>(rows: rusqlite::MappedRows<'_, F>) -> Result<Vec<T>>
where
    F: FnMut(&Row<'_>) -> rusqlite::Result<T>,
{
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub(crate) fn row_to_search_hit(row: &Row<'_>) -> rusqlite::Result<SearchHit> {
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

pub(crate) fn row_to_timeline_entry(
    row: &Row<'_>,
    is_anchor: bool,
) -> rusqlite::Result<TimelineEntry> {
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

pub(crate) fn estimate_hit_tokens(hit: &SearchHit) -> usize {
    let chars = hit.title.len() + hit.topic_key.len() + hit.snippet.len() + 32;
    chars.div_ceil(4)
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
