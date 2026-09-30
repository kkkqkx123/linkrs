//! Error delegation to the dedicated error statistics manager.
use std::collections::HashMap;

use crate::error_stats::{ErrorInfo, ErrorType, QueryPhase};
use crate::profile::QueryProfile;

use super::core::StatsManager;

impl StatsManager {
    pub fn record_error(&self, error_type: ErrorType, phase: QueryPhase) {
        self.error_stats.record_error(error_type, phase);
    }

    pub fn get_error_count(&self, error_type: ErrorType) -> u64 {
        self.error_stats.get_error_count(error_type)
    }

    pub fn get_error_count_by_phase(&self, phase: QueryPhase) -> u64 {
        self.error_stats.get_error_count_by_phase(phase)
    }

    pub fn get_all_error_counts(&self) -> HashMap<ErrorType, u64> {
        self.error_stats.get_all_error_counts()
    }

    pub fn get_all_error_counts_by_phase(&self) -> HashMap<QueryPhase, u64> {
        self.error_stats.get_all_error_counts_by_phase()
    }

    pub fn reset_error_counts(&self) {
        self.error_stats.reset_error_counts();
    }

    pub fn record_failed_query(&self, mut profile: QueryProfile, error_info: ErrorInfo) {
        profile.mark_failed_with_info(error_info.clone());
        self.error_stats
            .record_error_with_context(&error_info, Some(profile.query_text.clone()));
        self.record_query_profile(profile);
    }

    pub fn get_error_summary(&self) -> crate::error_stats::ErrorSummary {
        self.error_stats.get_error_summary()
    }

    pub fn get_recent_errors(&self, limit: usize) -> Vec<crate::error_stats::RecentError> {
        self.error_stats.get_recent_errors(limit)
    }
}
