use graphdb_metrics::StatsManager;
use std::sync::Arc;

use super::super::IndexDataManagerImpl;

impl IndexDataManagerImpl {
    pub fn set_stats_manager(&mut self, stats_manager: Arc<StatsManager>) {
        self.stats_manager = Some(stats_manager);
    }
}
