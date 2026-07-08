use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub fn calculate_cache_hit_rate(hits: u64, misses: u64) -> f64 {
    let total = hits + misses;
    if total > 0 {
        hits as f64 / total as f64
    } else {
        0.0
    }
}

#[derive(Debug, Default)]
pub struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    expirations: AtomicU64,
    insertions: AtomicU64,
    rejections: AtomicU64,
}

impl CacheStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_expiration(&self) {
        self.expirations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_insertion(&self) {
        self.insertions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_rejection(&self) {
        self.rejections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_hits(&self, count: u64) {
        self.hits.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_misses(&self, count: u64) {
        self.misses.fetch_add(count, Ordering::Relaxed);
    }

    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    pub fn expirations(&self) -> u64 {
        self.expirations.load(Ordering::Relaxed)
    }

    pub fn insertions(&self) -> u64 {
        self.insertions.load(Ordering::Relaxed)
    }

    pub fn rejections(&self) -> u64 {
        self.rejections.load(Ordering::Relaxed)
    }

    pub fn total_requests(&self) -> u64 {
        self.hits() + self.misses()
    }

    pub fn total(&self) -> u64 {
        self.hits() + self.misses()
    }

    pub fn hit_rate(&self) -> f64 {
        calculate_cache_hit_rate(self.hits(), self.misses())
    }

    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
        self.expirations.store(0, Ordering::Relaxed);
        self.insertions.store(0, Ordering::Relaxed);
        self.rejections.store(0, Ordering::Relaxed);
    }
}

impl Clone for CacheStats {
    fn clone(&self) -> Self {
        Self {
            hits: AtomicU64::new(self.hits()),
            misses: AtomicU64::new(self.misses()),
            evictions: AtomicU64::new(self.evictions()),
            expirations: AtomicU64::new(self.expirations()),
            insertions: AtomicU64::new(self.insertions()),
            rejections: AtomicU64::new(self.rejections()),
        }
    }
}

pub trait TimeConversion {
    fn as_micros(&self) -> u64;

    fn as_millis_f64(&self) -> f64 {
        self.as_micros() as f64 / 1000.0
    }

    fn as_seconds_f64(&self) -> f64 {
        self.as_micros() as f64 / 1_000_000.0
    }
}

impl TimeConversion for Duration {
    fn as_micros(&self) -> u64 {
        self.as_micros() as u64
    }
}

impl TimeConversion for u64 {
    fn as_micros(&self) -> u64 {
        *self
    }
}

pub fn calculate_average(total: f64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total / count as f64
    }
}

pub fn micros_to_millis(micros: u64) -> f64 {
    micros as f64 / 1000.0
}

pub fn duration_to_micros(duration: Duration) -> u64 {
    duration.as_micros() as u64
}

pub fn format_duration(micros: u64) -> String {
    if micros >= 1_000_000 {
        format!("{:.2}s", micros as f64 / 1_000_000.0)
    } else if micros >= 1_000 {
        format!("{:.2}ms", micros as f64 / 1_000.0)
    } else {
        format!("{}us", micros)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_cache_hit_rate() {
        assert_eq!(calculate_cache_hit_rate(90, 10), 0.9);
        assert_eq!(calculate_cache_hit_rate(0, 0), 0.0);
        assert_eq!(calculate_cache_hit_rate(100, 0), 1.0);
    }

    #[test]
    fn test_calculate_average() {
        assert_eq!(calculate_average(100.0, 10), 10.0);
        assert_eq!(calculate_average(0.0, 0), 0.0);
    }

    #[test]
    fn test_micros_to_millis() {
        assert_eq!(micros_to_millis(1000), 1.0);
        assert_eq!(micros_to_millis(500), 0.5);
        assert_eq!(micros_to_millis(0), 0.0);
    }

    #[test]
    fn test_duration_to_micros() {
        assert_eq!(duration_to_micros(Duration::from_micros(100)), 100);
        assert_eq!(duration_to_micros(Duration::from_millis(1)), 1000);
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(500), "500us");
        assert_eq!(format_duration(1500), "1.50ms");
        assert_eq!(format_duration(1_500_000), "1.50s");
    }
}
