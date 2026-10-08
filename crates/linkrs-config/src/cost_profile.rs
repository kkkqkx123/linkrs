//! Storage cost profile: picks the optimizer cost preset matching the
//! underlying storage medium.
//!
//! The query engine ships one cost preset per medium (`for_hdd` / `for_ssd`
//! / `for_in_memory` on the cost-model side); this module decides which one
//! applies. `Auto` probes the block device backing the data directory and
//! falls back to SSD when detection is impossible — modern deployments are
//! SSD by default, and the conservative HDD preset would systematically
//! misprice index scans.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Cost preset selector for the storage medium behind a deployment.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum StorageCostProfile {
    /// Probe the data directory's block device; SSD when undetectable.
    #[default]
    Auto,
    /// Mechanical hard drive preset.
    Hdd,
    /// Solid-state drive preset.
    Ssd,
    /// In-memory preset (no disk I/O costs).
    Memory,
}

impl StorageCostProfile {
    /// Resolve to a concrete preset.
    ///
    /// Explicit variants pass through. `Auto` probes `data_dir`; memory-mode
    /// deployments should map to [`StorageCostProfile::Memory`] before
    /// calling (see [`StorageCostProfile::resolve_for_runtime`]).
    pub fn resolve(&self, data_dir: &Path) -> Self {
        match self {
            Self::Auto => Self::detect_for_dir(data_dir),
            preset => *preset,
        }
    }

    /// Resolve with runtime knowledge: memory mode short-circuits to
    /// [`StorageCostProfile::Memory`] without touching the filesystem.
    pub fn resolve_for_runtime(&self, runtime: &crate::runtime::RuntimeConfig) -> Self {
        if runtime.is_memory() {
            return Self::Memory;
        }
        let dir = runtime.path().map(Path::new).unwrap_or(Path::new("."));
        self.resolve(dir)
    }

    /// Probe the block device backing `data_dir`.
    ///
    /// Returns [`StorageCostProfile::Hdd`] only on a positive rotational
    /// reading; every other outcome (SSD, virtual device, missing sysfs,
    /// non-Unix platform, any I/O error) is [`StorageCostProfile::Ssd`].
    pub fn detect_for_dir(data_dir: &Path) -> Self {
        if is_rotational(data_dir).unwrap_or(false) {
            Self::Hdd
        } else {
            Self::Ssd
        }
    }
}

/// Whether the device backing `path` reports itself rotational.
///
/// Positive-only signal: `Some(true)` requires reading `1` from the
/// device's `queue/rotational`; `None` means "unknown", never "SSD".
fn is_rotational(path: &Path) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // glibc makedev split, dependency-free.
        fn dev_major(dev: u64) -> u64 {
            ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff)
        }
        fn dev_minor(dev: u64) -> u64 {
            (dev & 0xff) | ((dev >> 12) & !0xff)
        }
        let dev = std::fs::metadata(path).ok()?.dev();
        let (major, minor) = (dev_major(dev), dev_minor(dev));
        // /sys/dev/block/<maj>:<min> symlinks into the device tree; walk up
        // until a queue/rotational attribute appears (partitions lack one).
        let mut current = std::fs::read_link(format!("/sys/dev/block/{major}:{minor}")).ok()?;
        for _ in 0..8 {
            let candidate = Path::new("/sys").join(&current).join("queue/rotational");
            if let Ok(text) = std::fs::read_to_string(&candidate) {
                return Some(text.trim() == "1");
            }
            if !current.pop() {
                break;
            }
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_presets_pass_through() {
        let dir = Path::new("/nonexistent-dir-for-test");
        assert_eq!(
            StorageCostProfile::Hdd.resolve(dir),
            StorageCostProfile::Hdd
        );
        assert_eq!(
            StorageCostProfile::Ssd.resolve(dir),
            StorageCostProfile::Ssd
        );
        assert_eq!(
            StorageCostProfile::Memory.resolve(dir),
            StorageCostProfile::Memory
        );
    }

    #[test]
    fn auto_falls_back_to_ssd_when_undetectable() {
        assert_eq!(
            StorageCostProfile::Auto.resolve(Path::new("/nonexistent-dir-for-test")),
            StorageCostProfile::Ssd
        );
    }

    #[test]
    fn rotational_probe_is_positive_only() {
        // The temp dir lives on an unknown device in CI: the probe must
        // return a definite reading or unknown, never error.
        let dir = std::env::temp_dir();
        let _ = is_rotational(&dir);
    }

    #[test]
    fn memory_runtime_short_circuits_detection() {
        let runtime = crate::runtime::RuntimeConfig::memory();
        assert_eq!(
            StorageCostProfile::Auto.resolve_for_runtime(&runtime),
            StorageCostProfile::Memory
        );
    }
}
