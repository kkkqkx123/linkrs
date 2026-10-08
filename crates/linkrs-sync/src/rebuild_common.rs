//! Shared rebuild coordination primitives for derived secondary indexes.
//!
//! Fulltext and vector rebuilds keep engine-specific scratch creation and
//! publish paths, but share admission, backfill batching, catch-up bounds,
//! empty-source guarding and progress accounting. This module holds that
//! shared core so both drivers use one policy entry point.

/// Catch-up and backfill tunables shared by all index rebuilds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebuildCommonOptions {
    /// Outbox events fetched per catch-up round.
    pub catchup_fetch_limit: u64,
    /// Non-final catch-up rounds before publish; the final drain always runs
    /// to the publish frontier.
    pub max_catchup_rounds: usize,
    /// Refuse to publish when the primary-storage source yields no documents.
    /// Publishing an empty rebuild over live data would delete servable data
    /// with nothing to backfill; aborting keeps the live index servable.
    pub allow_empty_source: bool,
}

impl Default for RebuildCommonOptions {
    fn default() -> Self {
        Self {
            catchup_fetch_limit: 1000,
            max_catchup_rounds: 10,
            allow_empty_source: false,
        }
    }
}

impl RebuildCommonOptions {
    pub fn new(
        catchup_fetch_limit: u64,
        max_catchup_rounds: usize,
        allow_empty_source: bool,
    ) -> Self {
        Self {
            catchup_fetch_limit,
            max_catchup_rounds,
            allow_empty_source,
        }
    }

    /// Guard against publishing an empty source without explicit opt-in.
    pub fn ensure_source_non_empty(&self, yielded: usize, index_desc: &str) -> Result<(), String> {
        if yielded == 0 && !self.allow_empty_source {
            return Err(format!(
                "rebuild source for {} yielded no documents; aborting to keep live data servable",
                index_desc
            ));
        }
        Ok(())
    }

    /// Admission key shared by all rebuild targets.
    pub fn lock_key(target: &str, space_id: u64, tag_name: &str, field_name: &str) -> String {
        format!("{target}:{space_id}:{tag_name}:{field_name}")
    }

    /// Number of delivery batches needed for a backfill of `total` docs.
    pub fn batch_count(total: usize, chunk: usize) -> usize {
        if chunk == 0 {
            return 0;
        }
        total.div_ceil(chunk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_source_by_default() {
        let options = RebuildCommonOptions::default();
        assert!(options.ensure_source_non_empty(0, "1.a.b").is_err());
        assert!(options.ensure_source_non_empty(3, "1.a.b").is_ok());
    }

    #[test]
    fn allows_empty_source_with_opt_in() {
        let options = RebuildCommonOptions::new(100, 2, true);
        assert!(options.ensure_source_non_empty(0, "1.a.b").is_ok());
    }

    #[test]
    fn lock_key_is_stable() {
        assert_eq!(
            RebuildCommonOptions::lock_key("vector", 1, "Tag", "field"),
            "vector:1:Tag:field"
        );
    }
}
