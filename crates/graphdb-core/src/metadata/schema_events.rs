use std::sync::Arc;

/// Schema change notification emitted after a DDL mutation succeeds.
///
/// Emission is best-effort: callbacks run synchronously on the DDL path,
/// isolated with `catch_unwind`, and must not affect the main flow.
/// The event reference is valid only for the dispatch call; observers must
/// not retain it beyond the callback.
#[derive(Debug, Clone)]
pub enum SchemaChangeEvent {
    SpaceCreated {
        space_id: u64,
        space_name: String,
    },
    SpaceDropped {
        space_id: u64,
        space_name: String,
    },
    SpaceUpdated {
        space_id: u64,
        space_name: String,
    },
    SpaceCleared {
        space_id: u64,
        space_name: String,
    },
    TagCreated {
        space_id: u64,
        space_name: String,
        tag_id: u32,
        tag_name: String,
    },
    TagAltered {
        space_id: u64,
        space_name: String,
        tag_id: u32,
        tag_name: String,
        added_properties: Vec<String>,
        removed_properties: Vec<String>,
    },
    TagDropped {
        space_id: u64,
        space_name: String,
        tag_id: u32,
        tag_name: String,
    },
    TagRenamed {
        space_id: u64,
        space_name: String,
        tag_id: u32,
        old_name: String,
        new_name: String,
    },
    EdgeTypeCreated {
        space_id: u64,
        space_name: String,
        edge_type_id: u32,
        type_name: String,
    },
    EdgeTypeAltered {
        space_id: u64,
        space_name: String,
        edge_type_id: u32,
        type_name: String,
        added_properties: Vec<String>,
        removed_properties: Vec<String>,
    },
    EdgeTypeDropped {
        space_id: u64,
        space_name: String,
        edge_type_id: u32,
        type_name: String,
    },
    EdgeTypeRenamed {
        space_id: u64,
        space_name: String,
        edge_type_id: u32,
        old_name: String,
        new_name: String,
    },
    SequenceCreated {
        name: String,
    },
    SequenceDropped {
        name: String,
    },
    SequenceAltered {
        name: String,
    },
    TagIndexCreated {
        space_id: u64,
        index_name: String,
    },
    TagIndexDropped {
        space_id: u64,
        index_name: String,
    },
    EdgeIndexCreated {
        space_id: u64,
        index_name: String,
    },
    EdgeIndexDropped {
        space_id: u64,
        index_name: String,
    },
}

/// Runtime observer for schema changes.
pub type SchemaChangeCallback = Arc<dyn Fn(&SchemaChangeEvent) + Send + Sync>;
