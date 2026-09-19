//! Group and leaf-region address arithmetic plus density calibration.
//!
//! Pure functions over the locked address width: group/region mapping,
//! address validation, and the calibrator-tree density model derived from
//! the packed-CSR target.

use graphdb_core::{StorageError, StorageResult};

use super::super::mutable_csr::PACKED_CSR_DENSITY;

/// Default address bits per node group: 12 bits cover 4096 rows.
pub const DEFAULT_NODE_GROUP_BITS: u32 = 12;
/// Rows per leaf region inside a group. A default group holds 16 regions;
/// region dirt and density merges operate at this granularity.
pub const LEAF_REGION_ROWS: usize = 256;

/// Group index for a global vertex id.
pub fn group_id_for(vid: u32, group_bits: u32) -> usize {
    (vid >> group_bits) as usize
}

/// First global vertex id covered by a group.
pub fn group_base(group: usize, group_bits: u32) -> u32 {
    (group as u32) << group_bits
}

/// Rows covered by one group.
pub fn group_size(group_bits: u32) -> usize {
    1usize << group_bits
}

/// Row address inside its group.
pub fn local_vid(vid: u32, group_bits: u32) -> u32 {
    vid & ((group_size(group_bits) as u32).wrapping_sub(1))
}

/// Validate the configured address width.
pub fn validate_group_bits(group_bits: u32) -> StorageResult<()> {
    if group_bits == 0 || group_bits > 20 {
        return Err(StorageError::invalid_operation(format!(
            "node_group_bits must be within 1..=20, got {}",
            group_bits
        )));
    }
    Ok(())
}

/// Leaf regions covering one group of `group_size` rows.
pub fn regions_per_group(group_size: usize) -> usize {
    group_size.div_ceil(LEAF_REGION_ROWS).max(1)
}

/// Leaf region owning a group-local row.
pub fn region_id_for_local(local: u32) -> usize {
    local as usize / LEAF_REGION_ROWS
}

/// Group-local `[start, end)` row window of one leaf region.
pub fn region_local_range(region: usize, group_size: usize) -> (u32, u32) {
    let start = (region * LEAF_REGION_ROWS).min(group_size) as u32;
    let end = ((region + 1) * LEAF_REGION_ROWS).min(group_size) as u32;
    (start, end)
}

/// Leaf-level density ceiling of the packed-CSR calibrator tree: leaf
/// regions may pack full, higher levels grade down toward the packed target.
pub const LEAF_HIGH_CSR_DENSITY: f32 = 1.0;

/// Height of the density calibrator tree for one address width: group rows
/// halve down to leaf regions, so the height is the group-bits minus the
/// leaf-region-rows log2.
pub fn calibrator_tree_height(group_bits: u32) -> u32 {
    group_bits.saturating_sub(LEAF_REGION_ROWS.trailing_zeros())
}

/// Density ceiling for one calibrator-tree level: full at the leaf level,
/// grading linearly down to the packed row target at the top. Levels above
/// the top clamp to the packed target.
pub fn calibrator_max_density(level: u32, tree_height: u32) -> f32 {
    if level == 0 || tree_height == 0 {
        return LEAF_HIGH_CSR_DENSITY;
    }
    let step = (LEAF_HIGH_CSR_DENSITY - PACKED_CSR_DENSITY) / tree_height as f32;
    PACKED_CSR_DENSITY + step * tree_height.saturating_sub(level.min(tree_height)) as f32
}

/// Reserved tail gap for a region holding `live` entries at the packed
/// density target. Empty regions hold no gap; gaps live at the region tail,
/// never inside rows.
pub fn region_tail_gap(live: usize) -> usize {
    if live == 0 {
        return 0;
    }
    ((live as f32 / PACKED_CSR_DENSITY).ceil() as usize).saturating_sub(live)
}
