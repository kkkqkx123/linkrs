use crate::core::DataType;
use std::collections::HashMap;
use std::sync::Arc;

pub type SlotId = usize;

#[derive(Debug, Clone)]
pub struct SlotInfo {
    pub slot_id: SlotId,
    pub name: String,
    pub data_type: Option<DataType>,
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
        Self { slots, name_to_slot }
    }

    pub fn from_names(names: &[String]) -> Self {
        let slots: Vec<SlotInfo> = names
            .iter()
            .enumerate()
            .map(|(i, name)| SlotInfo {
                slot_id: i,
                name: name.clone(),
                data_type: None,
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
                data_type: types.get(i).cloned().unwrap_or(None),
            })
            .collect();
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
            data_type: info.data_type.clone(),
        });
    }
    SlotLayout::new(slots)
}
