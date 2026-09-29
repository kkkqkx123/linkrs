//! Canonical storage-profile to cost-model mapping.
//!
//! Both the embedded constructors and the network server resolve a
//! [`StorageCostProfile`] first and then map the resolved preset onto a
//! [`CostModelConfig`]. The mapping lives here exactly once so the two
//! assembly paths cannot drift apart.

use std::collections::HashMap;

use graphdb_config::{RuntimeConfig, StorageCostProfile};
use graphdb_query::optimizer::CostModelConfig;

/// Map an already-resolved storage profile onto its cost preset.
///
/// Callers resolve `Auto` beforehand (`resolve` / `resolve_for_runtime`);
/// the `Auto` arm keeps the documented SSD default for direct callers.
pub fn cost_config_for_profile(profile: StorageCostProfile) -> CostModelConfig {
    match profile {
        StorageCostProfile::Hdd => CostModelConfig::for_hdd(),
        StorageCostProfile::Ssd | StorageCostProfile::Auto => CostModelConfig::for_ssd(),
        StorageCostProfile::Memory => CostModelConfig::for_in_memory(),
    }
}

/// Resolve every per-space profile override with runtime knowledge.
///
/// Each entry goes through the same resolution as the global profile, so a
/// memory-mode deployment short-circuits all spaces to the in-memory preset
/// and `Auto` entries reuse the data-directory probe. Returns the resolved
/// cost configs together with their display labels for EXPLAIN.
pub fn resolve_space_cost_configs(
    profiles: &HashMap<String, StorageCostProfile>,
    runtime: &RuntimeConfig,
) -> (HashMap<String, CostModelConfig>, HashMap<String, String>) {
    let mut configs = HashMap::with_capacity(profiles.len());
    let mut labels = HashMap::with_capacity(profiles.len());
    for (space, profile) in profiles {
        let resolved = profile.resolve_for_runtime(runtime);
        configs.insert(space.clone(), cost_config_for_profile(resolved));
        labels.insert(space.clone(), format!("{resolved:?}"));
    }
    (configs, labels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_map_to_matching_cost_configs() {
        assert_eq!(
            cost_config_for_profile(StorageCostProfile::Hdd).random_page_cost,
            CostModelConfig::for_hdd().random_page_cost
        );
        assert_eq!(
            cost_config_for_profile(StorageCostProfile::Ssd).random_page_cost,
            CostModelConfig::for_ssd().random_page_cost
        );
        assert_eq!(
            cost_config_for_profile(StorageCostProfile::Memory).random_page_cost,
            CostModelConfig::for_in_memory().random_page_cost
        );
    }

    #[test]
    fn auto_maps_to_ssd_default() {
        assert_eq!(
            cost_config_for_profile(StorageCostProfile::Auto).random_page_cost,
            CostModelConfig::for_ssd().random_page_cost
        );
    }
}
