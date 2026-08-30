use super::OptimizerEngine;
impl OptimizerEngine {
    pub fn maybe_apply_feedback(&self) {
        if !self.enable_feedback {
            return;
        }
        let history = self.feedback_history();
        let mut applied = 0usize;
        let mut decision_runs = 0usize;
        for fingerprint in history.get_all_fingerprints() {
            let Some(avg_error) = history.get_avg_row_error(&fingerprint) else {
                continue;
            };
            if !self.feedback_trigger.should_trigger(avg_error) {
                continue;
            }
            for feedback in history.get_feedback_for_query(&fingerprint) {
                let space = feedback.space.as_deref().unwrap_or("");
                if feedback.apply_rows > 0 {
                    self.decision_feedback.record_apply_run(
                        space,
                        feedback.apply_rows,
                        feedback.apply_time_us,
                    );
                    decision_runs += 1;
                }
                if feedback.join_rows > 0 {
                    self.decision_feedback.record_join_run(
                        space,
                        feedback.join_rows,
                        feedback.join_time_us,
                    );
                    decision_runs += 1;
                }
                for op in &feedback.operator_feedbacks {
                    if let Some(key) = &op.condition_key {
                        // Correction factor = actual_rows / estimated_rows.
                        let ratio = op.actual_rows as f64 / op.estimated_rows.max(1) as f64;
                        if self.selectivity_feedback.update_feedback_ratio(key, ratio) {
                            applied += 1;
                        }
                    } else if let Some(key) = &op.shape_key {
                        let ratio = op.actual_rows as f64 / op.estimated_rows.max(1) as f64;
                        if self.cardinality_feedback.update_feedback_ratio(key, ratio) {
                            applied += 1;
                        }
                    }
                }
            }
            self.feedback_trigger.mark_updated();
        }
        if applied > 0 {
            log::debug!(
                "Feedback loop: applied {} selectivity/cardinality corrections from {} fingerprints",
                applied,
                history.query_count()
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
