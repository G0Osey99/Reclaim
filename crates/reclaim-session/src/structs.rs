//! Lost-structure scan orchestration (docs/plan/03 §2.4, FR-SCAN-4): run the
//! [`reclaim_structs`] anchor sweep, cross-validate each proposal by probing the
//! metadata engines at the proposed offset, and persist the results in the
//! session's `proposed_volumes` table for `reclaim volumes` / `adopt`.

use crate::meta;
use crate::store::{ProposalRow, Store};
use crate::SessionError;
use reclaim_block::{BlockSource, OffsetView};
use std::sync::Arc;

/// Scan `src` for lost volumes, cross-validate, store and return the proposals.
pub fn scan_and_store(
    src: &Arc<dyn BlockSource>,
    store: &Store,
) -> Result<Vec<ProposalRow>, SessionError> {
    let mut rows = scan(src);
    store.replace_proposals(&rows)?;
    // Return in the stored (display) order.
    rows.shrink_to_fit();
    Ok(rows)
}

/// Scan + cross-validate without persisting (for callers that just want the list).
#[must_use]
pub fn scan(src: &Arc<dyn BlockSource>) -> Vec<ProposalRow> {
    let proposals = reclaim_structs::scan(src);
    let mut rows: Vec<ProposalRow> = Vec::with_capacity(proposals.len());
    for p in proposals {
        let mut fs = p.fs.clone();
        let mut conf = p.confidence;
        let mut evidence = p.evidence.clone();
        // Cross-validate: build the proposed window and see if an engine probes.
        if p.len > 0 {
            if let Ok(view) = OffsetView::new(Arc::clone(src), p.start, p.len) {
                let v: Arc<dyn BlockSource> = Arc::new(view);
                if let Some((kind, c)) = meta::probe_best(&v) {
                    fs = kind.to_string();
                    conf = conf.max(0.9).max(c);
                    evidence = format!("{evidence}; FS probe confirms {kind} ({c:.2})");
                }
            }
        }
        rows.push(ProposalRow {
            start: p.start,
            len: p.len,
            fs,
            confidence: conf,
            evidence,
        });
    }
    // Highest confidence first, then by offset (stable display / adopt order).
    rows.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.start.cmp(&b.start))
    });
    rows
}
