//! Cross-validation of linked memory observations against `SymbolFreshnessProvider`.

use anyhow::Result;
use anvil_core::traits::SymbolFreshnessProvider;

use crate::model::StalenessUpdate;
use crate::store::MemoryStore;

/// Evaluates all observations with `related_files` or `related_symbols` against `provider`.
pub fn evaluate_staleness(
    store: &mut MemoryStore,
    provider: &dyn SymbolFreshnessProvider,
) -> Result<Vec<StalenessUpdate>> {
    let candidates = store.list_with_relations()?;
    let mut updates = Vec::new();
    for obs in candidates {
        let report = provider.check_freshness(&obs.related_files, &obs.related_symbols)?;
        if report.state != obs.freshness || report.reason != obs.freshness_reason {
            store.update_freshness(&obs.id, report.state, report.reason.as_deref())?;
            updates.push(StalenessUpdate {
                id: obs.id,
                topic_key: obs.topic_key,
                previous: obs.freshness,
                current: report.state,
                reason: report.reason,
            });
        }
    }
    Ok(updates)
}
