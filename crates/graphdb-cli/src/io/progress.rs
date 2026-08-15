use std::time::Instant;

pub struct ProgressBar {
    total: usize,
    current: usize,
    start_time: Instant,
    width: usize,
    last_update: usize,
    quiet: bool,
}

impl ProgressBar {
    pub fn new(total: usize) -> Self {
        Self {
            total,
            current: 0,
            start_time: Instant::now(),
            width: 50,
            last_update: 0,
            quiet: false,
        }
    }

    pub fn with_quiet(mut self, quiet: bool) -> Self {
        self.quiet = quiet;
        self
    }

    pub fn update(&mut self, current: usize) {
        self.current = current;
        if current - self.last_update >= 100 || current == self.total {
            self.render();
            self.last_update = current;
        }
    }

    pub fn increment(&mut self) {
        self.current += 1;
        if self.current - self.last_update >= 100 || self.current == self.total {
            self.render();
            self.last_update = self.current;
        }
    }

    pub fn finish(&mut self) {
        if self.quiet {
            return;
        }
        self.current = self.total;
        self.render();
        eprintln!();
    }

    fn render(&self) {
        if self.quiet || self.total == 0 {
            return;
        }

        let percent = self.current as f64 / self.total as f64;
        let filled = (percent * self.width as f64) as usize;
        let empty = self.width.saturating_sub(filled);

        let elapsed = self.start_time.elapsed().as_secs();
        let rate = if elapsed > 0 {
            self.current as f64 / elapsed as f64
        } else {
            0.0
        };

        let eta = if rate > 0.0 {
            ((self.total - self.current) as f64 / rate) as u64
        } else {
            0
        };

        eprint!(
            "\r[{}{}] {}/{} ({:.1}%) {:.0} rows/s ETA: {}s   ",
            "=".repeat(filled),
            " ".repeat(empty),
            self.current,
            self.total,
            percent * 100.0,
            rate,
            eta
        );
    }

    pub fn set_total(&mut self, total: usize) {
        self.total = total;
    }
}

impl Drop for ProgressBar {
    fn drop(&mut self) {
        if !self.quiet {
            eprintln!();
        }
    }
}

pub fn format_progress(current: usize, total: usize, rate: f64) -> String {
    if total == 0 {
        return format!("{} rows ({:.0} rows/s)", current, rate);
    }

    let percent = current as f64 / total as f64 * 100.0;
    format!("{}/{} ({:.1}%) {:.0} rows/s", current, total, percent, rate)
}
