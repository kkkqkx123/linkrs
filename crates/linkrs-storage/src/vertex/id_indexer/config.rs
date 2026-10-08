//! Construction parameters for the primary-key index.

pub(super) const DEFAULT_INITIAL_CAPACITY: usize = 1024;
pub const DEFAULT_GROWTH_FACTOR: f64 = 1.5;
pub const MAX_CAPACITY: usize = u32::MAX as usize;

#[derive(Debug, Clone)]
pub struct IdIndexerConfig {
    pub initial_capacity: usize,
    pub growth_factor: f64,
    pub max_capacity: usize,
}

impl Default for IdIndexerConfig {
    fn default() -> Self {
        Self {
            initial_capacity: DEFAULT_INITIAL_CAPACITY,
            growth_factor: DEFAULT_GROWTH_FACTOR,
            max_capacity: MAX_CAPACITY,
        }
    }
}

impl IdIndexerConfig {
    pub fn with_initial_capacity(mut self, capacity: usize) -> Self {
        self.initial_capacity = capacity;
        self
    }
}
