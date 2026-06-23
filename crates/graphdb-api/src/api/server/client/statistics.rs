use crate::core::SessionStatistics;

#[derive(Debug)]
pub struct StatisticsContext {
    statistics: SessionStatistics,
}

impl Default for StatisticsContext {
    fn default() -> Self {
        Self::new()
    }
}

impl StatisticsContext {
    pub fn new() -> Self {
        Self {
            statistics: SessionStatistics::new(),
        }
    }

    pub fn statistics(&self) -> &SessionStatistics {
        &self.statistics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_statistics_context() {
        let context = StatisticsContext::new();
        assert_eq!(context.statistics().last_changes(), 0);
    }
}
