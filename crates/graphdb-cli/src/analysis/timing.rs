use std::time::{Duration, Instant};

pub struct QueryTimer {
    start: Instant,
    phases: Vec<(String, Duration)>,
}

impl QueryTimer {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            phases: Vec::new(),
        }
    }

    pub fn record_phase(&mut self, name: &str) {
        let elapsed = self.start.elapsed();
        let last = self
            .phases
            .last()
            .map(|(_, d)| *d)
            .unwrap_or(Duration::ZERO);
        let phase_time = elapsed - last;
        self.phases.push((name.to_string(), phase_time));
    }

    pub fn total_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    pub fn total_micros(&self) -> u128 {
        self.start.elapsed().as_micros()
    }

    pub fn phase_ms(&self, name: &str) -> Option<u64> {
        self.phases
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| d.as_millis() as u64)
    }

    pub fn phase_micros(&self, name: &str) -> Option<u128> {
        self.phases
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| d.as_micros())
    }

    pub fn format_time(&self) -> String {
        let total = self.total_ms();
        if total < 1000 {
            format!("Time: {:.3} ms", self.total_micros() as f64 / 1000.0)
        } else {
            format!("Time: {:.3} s", total as f64 / 1000.0)
        }
    }

    pub fn format_phases(&self) -> String {
        let mut output = String::new();
        output.push_str("Time Breakdown:\n");

        for (name, duration) in &self.phases {
            let ms = duration.as_millis();
            if ms > 0 {
                output.push_str(&format!("  {:20} {:.3} ms\n", name, ms as f64));
            }
        }

        output.push_str(&format!(
            "  {:20} {:.3} ms\n",
            "Total",
            self.total_ms() as f64
        ));
        output
    }

    pub fn reset(&mut self) {
        self.start = Instant::now();
        self.phases.clear();
    }
}

impl Default for QueryTimer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_timer_basic() {
        let mut timer = QueryTimer::new();
        sleep(Duration::from_millis(10));
        timer.record_phase("phase1");

        assert!(timer.total_ms() >= 10);
        assert!(timer.phase_ms("phase1").unwrap() >= 10);
    }

    #[test]
    fn test_timer_multiple_phases() {
        let mut timer = QueryTimer::new();
        sleep(Duration::from_millis(5));
        timer.record_phase("phase1");
        sleep(Duration::from_millis(5));
        timer.record_phase("phase2");

        assert!(timer.total_ms() >= 10);
        assert!(timer.phase_ms("phase1").unwrap() >= 5);
        assert!(timer.phase_ms("phase2").unwrap() >= 5);
    }
}
