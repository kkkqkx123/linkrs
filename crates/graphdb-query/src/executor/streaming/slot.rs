use graphdb_core::DataType;
use std::collections::HashMap;

pub type SlotId = usize;

/// Optional origin information for a slot (which plan node produced it).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SlotOrigin {
    /// Column from a storage scan
    StorageColumn(String),
    /// Computed expression output
    Computed,
    /// System-generated column (e.g. _vid, _expand_vertex)
    System,
    /// Join output column
    JoinOutput,
    /// Aggregation output
    AggregationOutput,
    /// Traversal result (vertex/edge/path)
    TraversalResult,
}

#[derive(Debug, Clone)]
pub struct SlotInfo {
    pub slot_id: SlotId,
    pub name: String,
    /// Optional alias (e.g. AS alias_name)
    pub alias: Option<String>,
    pub data_type: Option<DataType>,
    /// Whether the column can contain NULL values
    pub nullable: bool,
    /// Where this slot originates from
    pub origin: Option<SlotOrigin>,
}

#[derive(Debug, Clone)]
pub struct SlotLayout {
    pub slots: Vec<SlotInfo>,
    pub name_to_slot: HashMap<String, SlotId>,
}

impl SlotLayout {
    pub fn new(slots: Vec<SlotInfo>) -> Self {
        let name_to_slot = slots
            .iter()
            .enumerate()
            .map(|(i, info)| (info.name.clone(), i))
            .collect();
        Self {
            slots,
            name_to_slot,
        }
    }

    pub fn from_names(names: &[String]) -> Self {
        let slots: Vec<SlotInfo> = names
            .iter()
            .enumerate()
            .map(|(i, name)| SlotInfo {
                slot_id: i,
                name: name.clone(),
                alias: None,
                data_type: None,
                nullable: true,
                origin: None,
            })
            .collect();
        Self::new(slots)
    }

    pub fn from_names_and_types(names: &[String], types: &[Option<DataType>]) -> Self {
        let slots: Vec<SlotInfo> = names
            .iter()
            .enumerate()
            .map(|(i, name)| SlotInfo {
                slot_id: i,
                name: name.clone(),
                alias: None,
                data_type: types.get(i).cloned().unwrap_or(None),
                nullable: true,
                origin: None,
            })
            .collect();
        Self::new(slots)
    }

    /// Create a layout with full slot metadata.
    pub fn from_slots(slots: Vec<SlotInfo>) -> Self {
        Self::new(slots)
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn slot_id(&self, name: &str) -> Option<SlotId> {
        self.name_to_slot.get(name).copied()
    }

    pub fn slot_info(&self, id: SlotId) -> Option<&SlotInfo> {
        self.slots.get(id)
    }

    /// Return the names of all slots (useful for legacy code paths).
    pub fn names(&self) -> Vec<String> {
        self.slots.iter().map(|s| s.name.clone()).collect()
    }

    /// Resolve a slot by name, with alias fallback.
    pub fn resolve(&self, name_or_alias: &str) -> Option<SlotId> {
        // First try exact name match
        if let Some(&id) = self.name_to_slot.get(name_or_alias) {
            return Some(id);
        }
        // Then try alias match
        self.slots
            .iter()
            .find(|s| s.alias.as_deref() == Some(name_or_alias))
            .map(|s| s.slot_id)
    }
}

/// Combine two slot layouts side-by-side (for joins).
/// Left slots preserve their IDs; right slots get new IDs offset by left count.
pub fn combine_layouts(left: &SlotLayout, right: &SlotLayout) -> SlotLayout {
    let mut slots = left.slots.clone();
    let offset = slots.len();
    for info in &right.slots {
        slots.push(SlotInfo {
            slot_id: info.slot_id + offset,
            name: info.name.clone(),
            alias: info.alias.clone(),
            data_type: info.data_type.clone(),
            nullable: info.nullable,
            origin: info.origin.clone(),
        });
    }
    SlotLayout::new(slots)
}

/// Resolve naming conflicts in a combined layout by appending suffixes.
pub fn combine_layouts_with_dedup(left: &SlotLayout, right: &SlotLayout) -> SlotLayout {
    let mut slots = left.slots.clone();
    let offset = slots.len();
    let left_names: std::collections::HashSet<&str> =
        left.slots.iter().map(|s| s.name.as_str()).collect();

    for info in &right.slots {
        let name = if left_names.contains(info.name.as_str()) {
            format!("{}_{}", info.name, "right")
        } else {
            info.name.clone()
        };
        slots.push(SlotInfo {
            slot_id: info.slot_id + offset,
            name,
            alias: info.alias.clone(),
            data_type: info.data_type.clone(),
            nullable: info.nullable,
            origin: info.origin.clone(),
        });
    }
    SlotLayout::new(slots)
}
