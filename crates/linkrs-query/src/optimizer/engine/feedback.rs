use super::OptimizerEngine;
use crate::optimizer::stats::feedback::query::OperatorFeedback;
impl OptimizerEngine {
    /// Fold execution feedback into selectivity/cardinality corrections.
    ///
    /// Consumes actual row counts from `feedback_history`, which execution
    /// populates via `collect_execution_feedback` from runtime operator
    /// profiles (estimated vs actual per shape/predicate). Observability
    /// portraits (`QueryProfile` in metrics) and optimizer corrections
    /// (`QueryExecutionFeedback` here) serve separate roles — human-facing
    /// diagnostics vs EWMA estimate correction — and are both fed from the
    /// same runtime profiles.
    ///
    /// Each execution is folded exactly once (per-fingerprint sequence
    /// watermark): re-applying the retained history on every optimization
    /// would walk the corrections over identical samples again. Samples
    /// from starved operators or without a measurable input pairing are
    /// dropped instead of folded.
    pub fn maybe_apply_feedback(&self) {
        if !self.enable_feedback {
            return;
        }
        let history = self.feedback_history();
        let mut applied = 0usize;
        let mut skipped_truncated = 0usize;
        let mut skipped_unpaired = 0usize;
        let mut decision_runs = 0usize;
        for fingerprint in history.get_all_fingerprints() {
            let Some(avg_error) = history.get_avg_row_error(&fingerprint) else {
                continue;
            };
            if !self.feedback_trigger.should_trigger(avg_error) {
                continue;
            }
            let watermark = self
                .feedback_applied_seq
                .lock()
                .get(&fingerprint)
                .copied()
                .unwrap_or(0);
            let mut max_seq = watermark;
            let mut folded_runs = 0usize;
            for feedback in history.get_feedback_for_query(&fingerprint) {
                if feedback.sequence <= watermark {
                    continue;
                }
                max_seq = max_seq.max(feedback.sequence);
                if feedback.apply_rows > 0 {
                    self.decision_feedback.record_apply_run(
                        space_of(&feedback),
                        feedback.apply_rows,
                        feedback.apply_time_us,
                    );
                    decision_runs += 1;
                }
                if feedback.join_rows > 0 {
                    self.decision_feedback.record_join_run(
                        space_of(&feedback),
                        feedback.join_rows,
                        feedback.join_time_us,
                    );
                    decision_runs += 1;
                }
                for op in &feedback.operator_feedbacks {
                    if !op.exhausted {
                        // Starved by Limit / early termination: partial
                        // counts describe the cutoff, not the operator.
                        skipped_truncated += 1;
                        continue;
                    }
                    if let Some(key) = &op.condition_key {
                        match self.measured_selectivity_ratio(key, op) {
                            Some((ratio, input_rows)) => {
                                if self
                                    .selectivity_feedback
                                    .update_measured_ratio(key, ratio, input_rows)
                                {
                                    applied += 1;
                                }
                            }
                            None => {
                                skipped_unpaired += 1;
                            }
                        }
                    } else if let Some(key) = &op.shape_key {
                        let ratio = op.actual_rows as f64 / op.estimated_rows.max(1) as f64;
                        if self.cardinality_feedback.update_feedback_ratio(key, ratio) {
                            applied += 1;
                        }
                    }
                }
                self.feedback_trigger.record_feedback();
                folded_runs += 1;
            }
            if folded_runs > 0 {
                self.feedback_applied_seq
                    .lock()
                    .insert(fingerprint, max_seq);
                self.feedback_trigger.mark_updated();
            }
        }
        if applied > 0 {
            log::debug!(
                "Feedback loop: applied {} selectivity/cardinality corrections from {} fingerprints ({} truncated, {} unpaired)",
                applied,
                history.query_count(),
                skipped_truncated,
                skipped_unpaired,
            );
        }
        if decision_runs > 0 {
            log::debug!(
                "Feedback loop: folded {} Apply/Join decision runs from {} fingerprints",
                decision_runs,
                history.query_count()
            );
        }
    }

    /// Isolate the operator-local selectivity error for a filter sample.
    ///
    /// Returns the observed-vs-estimated selectivity ratio plus the measured
    /// input rows. `None` when the pairing is unavailable (unknown key,
    /// non-positive baseline, or missing/empty input measurement): such a
    /// sample cannot separate predicate error from upstream cardinality
    /// error and must not train the correction.
    fn measured_selectivity_ratio(&self, key: &str, op: &OperatorFeedback) -> Option<(f64, u64)> {
        let baseline = self
            .selectivity_feedback
            .get_feedback(key)?
            .estimated_selectivity();
        if baseline <= 0.0 {
            return None;
        }
        let actual_in = op.actual_input_rows?;
        if actual_in == 0 {
            return None;
        }
        let observed = op.actual_rows as f64 / actual_in as f64;
        Some((observed / baseline, actual_in))
    }

    /// Drop all feedback corrections scoped to `space` (`None` clears all).
    ///
    /// Called when a space's statistics are invalidated (ANALYZE force or
    /// DDL commit) so stale corrections cannot mislead estimates after a
    /// schema or data change.
    pub fn invalidate_space_feedback(&self, space: Option<&str>) {
        let removed = match space {
            Some(space) => {
                let removed = self
                    .selectivity_feedback
                    .remove_feedback_by_space(&format!("{}:", space));
                removed
                    + self
                        .cardinality_feedback
                        .remove_feedback_by_space(&format!("{}:", space))
            }
            None => {
                let keys = self.selectivity_feedback.get_all_keys();
                for key in &keys {
                    self.selectivity_feedback.remove_feedback(key);
                }
                let cardinality_keys = self.cardinality_feedback.get_all_keys();
                for key in &cardinality_keys {
                    self.cardinality_feedback.remove_feedback(key);
                }
                keys.len() + cardinality_keys.len()
            }
        };
        self.decision_feedback.invalidate_space(space);
        if removed > 0 {
            log::info!(
                "Invalidated {} feedback corrections for space {:?}",
                removed,
                space
            );
        }
    }
}

/// Target space of a collected execution, defaulting to unscoped.
fn space_of(feedback: &crate::optimizer::stats::feedback::query::QueryExecutionFeedback) -> &str {
    feedback.space.as_deref().unwrap_or("")
}
