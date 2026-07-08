use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutorStats {
    pub num_rows: usize,
    pub exec_time_us: u64,
    pub total_time_us: u64,
    pub memory_peak: usize,
    pub memory_current: usize,
    pub batch_count: usize,
    pub other_stats: HashMap<String, String>,
}

impl ExecutorStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_row(&mut self, count: usize) {
        self.num_rows += count;
    }

    pub fn add_exec_time(&mut self, duration: Duration) {
        self.exec_time_us += duration.as_micros() as u64;
    }

    pub fn add_total_time(&mut self, duration: Duration) {
        self.total_time_us += duration.as_micros() as u64;
    }

    pub fn set_memory_peak(&mut self, peak: usize) {
        if peak > self.memory_peak {
            self.memory_peak = peak;
        }
    }

    pub fn update_memory_current(&mut self, current: usize) {
        self.memory_current = current;
    }

    pub fn add_batch(&mut self, count: usize) {
        self.batch_count += count;
    }

    pub fn add_stat(&mut self, key: String, value: String) {
        self.other_stats.insert(key, value);
    }

    pub fn get_stat(&self, key: &str) -> Option<&String> {
        self.other_stats.get(key)
    }

    pub fn throughput_rows_per_sec(&self) -> f64 {
        if self.exec_time_us == 0 {
            return 0.0;
        }
        self.num_rows as f64 / (self.exec_time_us as f64 / 1_000_000.0)
    }

    pub fn efficiency_rows_per_us(&self) -> f64 {
        if self.exec_time_us == 0 {
            return 0.0;
        }
        self.num_rows as f64 / self.exec_time_us as f64
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn to_formatted_string(&self) -> String {
        format!(
            "rows={}, exec_time={}ms, total_time={}ms, memory_peak={}B, batches={}",
            self.num_rows,
            self.exec_time_us / 1000,
            self.total_time_us / 1000,
            self.memory_peak,
            self.batch_count,
        )
    }
}
