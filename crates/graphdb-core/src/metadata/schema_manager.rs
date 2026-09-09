use super::schema_events::{SchemaChangeCallback, SchemaChangeEvent};
use crate::event_dispatch::{EventFilter, EventSubscriptions, SubscriptionId};
use crate::metadata::sequence::SequenceDef;
use crate::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo};
use crate::StorageError;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

const SCHEMA_FORMAT_VERSION: u32 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct SchemaSnapshot {
    version: u32,
    spaces: Vec<SpaceInfo>,
    tags: Vec<(u64, TagInfo)>,
    edge_types: Vec<(u64, EdgeTypeInfo)>,
    space_id_counter: u64,
    tag_id_counters: Vec<(u64, u32)>,
    edge_type_id_counters: Vec<(u64, u32)>,
    /// Persisted SERIAL counters: (space_id, table name, next value).
    #[serde(default)]
    serial_next: Vec<(u64, String, u64)>,
    /// Persisted sequence definitions.
    #[serde(default)]
    sequences: Vec<SequenceDef>,
}

#[derive(Debug, Clone)]
struct SpaceData {
    info: SpaceInfo,
}

#[derive(Debug, Clone)]
struct TagData {
    info: TagInfo,
}

#[derive(Debug, Clone)]
struct EdgeTypeData {
    info: EdgeTypeInfo,
}

pub struct SchemaManager {
    spaces: Arc<RwLock<HashMap<u64, SpaceData>>>,
    space_name_index: Arc<RwLock<HashMap<String, u64>>>,
    tags: Arc<RwLock<HashMap<(u64, u32), TagData>>>,
    edge_types: Arc<RwLock<HashMap<(u64, u32), EdgeTypeData>>>,
    space_id_counter: Arc<AtomicU64>,
    tag_id_counter: Arc<DashMap<u64, AtomicU32>>,
    edge_type_id_counter: Arc<DashMap<u64, AtomicU32>>,
    serial_next: Arc<RwLock<Vec<(u64, String, u64)>>>,
    sequences: Arc<RwLock<HashMap<String, SequenceDef>>>,
    schema_callbacks: Arc<EventSubscriptions<SchemaChangeEvent>>,
}

impl Clone for SchemaManager {
    fn clone(&self) -> Self {
        Self {
            spaces: self.spaces.clone(),
            space_name_index: self.space_name_index.clone(),
            tags: self.tags.clone(),
            edge_types: self.edge_types.clone(),
            space_id_counter: self.space_id_counter.clone(),
            tag_id_counter: self.tag_id_counter.clone(),
            edge_type_id_counter: self.edge_type_id_counter.clone(),
            serial_next: self.serial_next.clone(),
            sequences: self.sequences.clone(),
            schema_callbacks: self.schema_callbacks.clone(),
        }
    }
}

impl std::fmt::Debug for SchemaManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchemaManager")
            .field("spaces_count", &self.spaces.read().len())
            .finish()
    }
}

impl SchemaManager {
    pub fn new() -> Self {
        Self {
            spaces: Arc::new(RwLock::new(HashMap::new())),
            space_name_index: Arc::new(RwLock::new(HashMap::new())),
            tags: Arc::new(RwLock::new(HashMap::new())),
            edge_types: Arc::new(RwLock::new(HashMap::new())),
            space_id_counter: Arc::new(AtomicU64::new(0)),
            tag_id_counter: Arc::new(DashMap::new()),
            edge_type_id_counter: Arc::new(DashMap::new()),
            serial_next: Arc::new(RwLock::new(Vec::new())),
            sequences: Arc::new(RwLock::new(HashMap::new())),
            schema_callbacks: Arc::new(EventSubscriptions::new()),
        }
    }

    /// Register a runtime observer for schema changes.
    pub fn register_schema_callback(&self, callback: SchemaChangeCallback) -> SubscriptionId {
        self.schema_callbacks.add(callback)
    }

    /// Register a filtered observer invoked only when `filter` returns true.
    pub fn register_schema_callback_filtered(
        &self,
        callback: SchemaChangeCallback,
        filter: EventFilter<SchemaChangeEvent>,
    ) -> SubscriptionId {
        self.schema_callbacks.add_filtered(callback, Some(filter))
    }

    /// Remove a previously registered observer. Returns true if present.
    pub fn unregister_schema_callback(&self, id: SubscriptionId) -> bool {
        self.schema_callbacks.remove(id)
    }

    /// Number of registered schema observers (useful for tests and diagnostics).
    pub fn schema_callback_count(&self) -> usize {
        self.schema_callbacks.len()
    }

    fn emit_schema_event(&self, event: SchemaChangeEvent) {
        self.schema_callbacks.dispatch("schema", &event);
    }

    /// Set the persisted SERIAL counters (space_id, table name, next value).
    /// Called by the storage layer before the schema snapshot is saved.
    pub fn set_serial_next(&self, entries: Vec<(u64, String, u64)>) {
        *self.serial_next.write() = entries;
    }

    /// Persisted SERIAL counters as (space_id, table name, next value) triples.
    pub fn serial_next(&self) -> Vec<(u64, String, u64)> {
        self.serial_next.read().clone()
    }

    fn get_next_space_id(&self) -> u64 {
        self.space_id_counter.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn peek_next_space_id(&self) -> u64 {
        self.space_id_counter.load(Ordering::SeqCst) + 1
    }

    fn get_next_tag_id(&self, _space_id: u64) -> u32 {
        let entry = self
            .tag_id_counter
            .entry(0)
            .or_insert_with(|| AtomicU32::new(0));
        entry.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn peek_next_tag_id(&self) -> u32 {
        let entry = self
            .tag_id_counter
            .entry(0)
            .or_insert_with(|| AtomicU32::new(0));
        entry.load(Ordering::SeqCst) + 1
    }

    fn get_next_edge_type_id(&self, _space_id: u64) -> u32 {
        let entry = self
            .edge_type_id_counter
            .entry(0)
            .or_insert_with(|| AtomicU32::new(0));
        entry.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn peek_next_edge_type_id(&self) -> u32 {
        let entry = self
            .edge_type_id_counter
            .entry(0)
            .or_insert_with(|| AtomicU32::new(0));
        entry.load(Ordering::SeqCst) + 1
    }

    pub fn create_space(&self, space: &mut SpaceInfo) -> Result<bool, StorageError> {
        let mut name_index = self.space_name_index.write();
        if name_index.contains_key(&space.space_name) {
            return Ok(false);
        }

        let space_id = if space.space_id == 0 {
            self.get_next_space_id()
        } else {
            let current = self.space_id_counter.load(Ordering::SeqCst);
            if space.space_id > current {
                self.space_id_counter
                    .store(space.space_id, Ordering::SeqCst);
            }

            let spaces = self.spaces.read();
            if spaces.contains_key(&space.space_id) {
                return Err(StorageError::label_already_exists(format!(
                    "space_id {}",
                    space.space_id
                )));
            }
            space.space_id
        };
        space.space_id = space_id;

        name_index.insert(space.space_name.clone(), space_id);
        drop(name_index);

        let mut spaces = self.spaces.write();
        spaces.insert(
            space_id,
            SpaceData {
                info: space.clone(),
            },
        );
        drop(spaces);
        let space_name = space.space_name.clone();
        self.emit_schema_event(SchemaChangeEvent::SpaceCreated {
            space_id,
            space_name,
        });

        Ok(true)
    }

    pub fn drop_space(&self, space_name: &str) -> Result<bool, StorageError> {
        let mut name_index = self.space_name_index.write();
        if let Some(space_id) = name_index.remove(space_name) {
            drop(name_index);

            let mut spaces = self.spaces.write();
            spaces.remove(&space_id);
            drop(spaces);

            let mut tags = self.tags.write();
            tags.retain(|(sid, _), _| *sid != space_id);
            drop(tags);

            let mut edge_types = self.edge_types.write();
            edge_types.retain(|(sid, _), _| *sid != space_id);
            drop(edge_types);

            self.emit_schema_event(SchemaChangeEvent::SpaceDropped {
                space_id,
                space_name: space_name.to_string(),
            });

            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn clear_space(&self, space_name: &str) -> Result<bool, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let space_id = space_info.space_id;

        let mut tags = self.tags.write();
        tags.retain(|(sid, _), _| *sid != space_id);
        drop(tags);

        let mut edge_types = self.edge_types.write();
        edge_types.retain(|(sid, _), _| *sid != space_id);
        drop(edge_types);

        self.emit_schema_event(SchemaChangeEvent::SpaceCleared {
            space_id,
            space_name: space_name.to_string(),
        });

        Ok(true)
    }

    pub fn alter_space_comment(
        &self,
        space_id: u64,
        comment: String,
    ) -> Result<bool, StorageError> {
        let (applied, space_name) = {
            let mut spaces = self.spaces.write();
            if let Some(data) = spaces.get_mut(&space_id) {
                data.info.comment = Some(comment);
                let name = data.info.space_name.clone();
                (true, name)
            } else {
                (false, String::new())
            }
        };
        if applied {
            self.emit_schema_event(SchemaChangeEvent::SpaceUpdated {
                space_id,
                space_name,
            });
        }
        Ok(applied)
    }

    pub fn get_space(&self, space_name: &str) -> Result<Option<SpaceInfo>, StorageError> {
        let name_index = self.space_name_index.read();
        if let Some(space_id) = name_index.get(space_name) {
            let spaces = self.spaces.read();
            if let Some(data) = spaces.get(space_id) {
                return Ok(Some(data.info.clone()));
            }
        }
        Ok(None)
    }

    pub fn get_space_id(&self, space_name: &str) -> Result<u64, StorageError> {
        let name_index = self.space_name_index.read();
        if let Some(space_id) = name_index.get(space_name) {
            Ok(*space_id)
        } else {
            Err(StorageError::db_error(format!(
                "Space \"{}\" does not exist",
                space_name
            )))
        }
    }

    pub fn get_space_by_id(&self, space_id: u64) -> Result<Option<SpaceInfo>, StorageError> {
        let spaces = self.spaces.read();
        Ok(spaces.get(&space_id).map(|d| d.info.clone()))
    }

    pub fn list_spaces(&self) -> Result<Vec<SpaceInfo>, StorageError> {
        let spaces = self.spaces.read();
        Ok(spaces.values().map(|d| d.info.clone()).collect())
    }

    pub fn update_space(&self, space: &SpaceInfo) -> Result<bool, StorageError> {
        let applied = {
            let mut spaces = self.spaces.write();
            if let std::collections::hash_map::Entry::Occupied(mut e) = spaces.entry(space.space_id)
            {
                e.insert(SpaceData {
                    info: space.clone(),
                });
                true
            } else {
                false
            }
        };
        if applied {
            self.emit_schema_event(SchemaChangeEvent::SpaceUpdated {
                space_id: space.space_id,
                space_name: space.space_name.clone(),
            });
        }
        Ok(applied)
    }

    pub fn create_tag(&self, space_name: &str, tag: &TagInfo) -> Result<u32, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let existing_tags = self.list_tags(space_name)?;
        if existing_tags.iter().any(|t| t.tag_name == tag.tag_name) {
            return Err(StorageError::label_already_exists(tag.tag_name.clone()));
        }

        let tag_id = self.get_next_tag_id(space_info.space_id);
        let mut tag_with_id = tag.clone();
        tag_with_id.tag_id = tag_id;

        {
            let mut tags = self.tags.write();
            tags.insert((space_info.space_id, tag_id), TagData { info: tag_with_id });
        }

        self.emit_schema_event(SchemaChangeEvent::TagCreated {
            space_id: space_info.space_id,
            space_name: space_name.to_string(),
            tag_id,
            tag_name: tag.tag_name.clone(),
        });

        Ok(tag_id)
    }

    pub fn create_tag_with_id(
        &self,
        space_name: &str,
        tag: &TagInfo,
        tag_id: u32,
    ) -> Result<u32, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let existing_tags = self.list_tags(space_name)?;
        if existing_tags
            .iter()
            .any(|existing| existing.tag_name == tag.tag_name)
        {
            return Err(StorageError::label_already_exists(tag.tag_name.clone()));
        }

        let mut tag_with_id = tag.clone();
        tag_with_id.tag_id = tag_id;

        {
            let mut tags = self.tags.write();
            tags.insert((space_info.space_id, tag_id), TagData { info: tag_with_id });
        }

        let entry = self
            .tag_id_counter
            .entry(0)
            .or_insert_with(|| AtomicU32::new(0));
        let current = entry.load(Ordering::SeqCst);
        if tag_id > current {
            entry.store(tag_id, Ordering::SeqCst);
        }

        self.emit_schema_event(SchemaChangeEvent::TagCreated {
            space_id: space_info.space_id,
            space_name: space_name.to_string(),
            tag_id,
            tag_name: tag.tag_name.clone(),
        });

        Ok(tag_id)
    }

    pub fn drop_tag(&self, space_name: &str, tag_name: &str) -> Result<bool, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let removed = {
            let mut tags = self.tags.write();
            let tag_key = tags
                .iter()
                .find(|((sid, _), data)| {
                    *sid == space_info.space_id && data.info.tag_name == tag_name
                })
                .map(|(k, _)| *k);

            if let Some(key) = tag_key {
                let removed = tags.remove(&key);
                removed.map(|d| d.info.tag_id)
            } else {
                None
            }
        };

        if let Some(tag_id) = removed {
            self.emit_schema_event(SchemaChangeEvent::TagDropped {
                space_id: space_info.space_id,
                space_name: space_name.to_string(),
                tag_id,
                tag_name: tag_name.to_string(),
            });
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn get_tag(
        &self,
        space_name: &str,
        tag_name: &str,
    ) -> Result<Option<TagInfo>, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let tags = self.tags.read();
        Ok(tags
            .iter()
            .find(|((sid, _), data)| *sid == space_info.space_id && data.info.tag_name == tag_name)
            .map(|(_, data)| data)
            .map(|d| d.info.clone()))
    }

    pub fn list_tags(&self, space_name: &str) -> Result<Vec<TagInfo>, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let tags = self.tags.read();
        Ok(tags
            .iter()
            .filter(|((sid, _), _)| *sid == space_info.space_id)
            .map(|(_, data)| data.info.clone())
            .collect())
    }

    pub fn find_tag_by_id(&self, tag_id: u32) -> Option<(String, TagInfo)> {
        let tags = self.tags.read();
        let spaces = self.spaces.read();

        tags.iter().find_map(|((space_id, current_tag_id), data)| {
            if *current_tag_id != tag_id {
                return None;
            }

            let space_name = spaces
                .get(space_id)
                .map(|space| space.info.space_name.clone())?;

            Some((space_name, data.info.clone()))
        })
    }

    pub fn update_tag(&self, space_name: &str, tag: &TagInfo) -> Result<bool, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let (applied, tag_id) = {
            let mut tags = self.tags.write();
            let tag_key = tags
                .iter()
                .find(|((sid, _), data)| {
                    *sid == space_info.space_id && data.info.tag_name == tag.tag_name
                })
                .map(|(k, _)| *k);

            if let Some(key) = tag_key {
                if let Some(data) = tags.get_mut(&key) {
                    let tag_id = data.info.tag_id;
                    data.info = tag.clone();
                    (true, tag_id)
                } else {
                    (false, 0)
                }
            } else {
                (false, 0)
            }
        };
        if applied {
            self.emit_schema_event(SchemaChangeEvent::TagAltered {
                space_id: space_info.space_id,
                space_name: space_name.to_string(),
                tag_id,
                tag_name: tag.tag_name.clone(),
                added_properties: tag.properties.iter().map(|p| p.name.clone()).collect(),
                removed_properties: Vec::new(),
            });
        }
        Ok(applied)
    }

    pub fn create_edge_type(
        &self,
        space_name: &str,
        edge_type: &EdgeTypeInfo,
    ) -> Result<u32, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let existing = self.list_edge_types(space_name)?;
        if existing
            .iter()
            .any(|e| e.edge_type_name == edge_type.edge_type_name)
        {
            return Err(StorageError::label_already_exists(
                edge_type.edge_type_name.clone(),
            ));
        }

        let edge_type_id = self.get_next_edge_type_id(space_info.space_id);
        let mut edge_with_id = edge_type.clone();
        edge_with_id.edge_type_id = edge_type_id;

        {
            let mut edge_types = self.edge_types.write();
            edge_types.insert(
                (space_info.space_id, edge_type_id),
                EdgeTypeData { info: edge_with_id },
            );
        }

        self.emit_schema_event(SchemaChangeEvent::EdgeTypeCreated {
            space_id: space_info.space_id,
            space_name: space_name.to_string(),
            edge_type_id,
            type_name: edge_type.edge_type_name.clone(),
        });

        Ok(edge_type_id)
    }

    pub fn create_edge_type_with_id(
        &self,
        space_name: &str,
        edge_type: &EdgeTypeInfo,
        edge_type_id: u32,
    ) -> Result<u32, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let existing = self.list_edge_types(space_name)?;
        if existing
            .iter()
            .any(|e| e.edge_type_name == edge_type.edge_type_name)
        {
            return Err(StorageError::label_already_exists(
                edge_type.edge_type_name.clone(),
            ));
        }

        let mut edge_with_id = edge_type.clone();
        edge_with_id.edge_type_id = edge_type_id;

        {
            let mut edge_types = self.edge_types.write();
            edge_types.insert(
                (space_info.space_id, edge_type_id),
                EdgeTypeData { info: edge_with_id },
            );
        }

        let entry = self
            .edge_type_id_counter
            .entry(0)
            .or_insert_with(|| AtomicU32::new(0));
        let current = entry.load(Ordering::SeqCst);
        if edge_type_id > current {
            entry.store(edge_type_id, Ordering::SeqCst);
        }

        self.emit_schema_event(SchemaChangeEvent::EdgeTypeCreated {
            space_id: space_info.space_id,
            space_name: space_name.to_string(),
            edge_type_id,
            type_name: edge_type.edge_type_name.clone(),
        });

        Ok(edge_type_id)
    }

    pub fn drop_edge_type(
        &self,
        space_name: &str,
        edge_type_name: &str,
    ) -> Result<bool, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let removed: Option<(u32, String)> = {
            let mut edge_types = self.edge_types.write();
            let key = edge_types
                .iter()
                .find(|((sid, _), data)| {
                    *sid == space_info.space_id && data.info.edge_type_name == edge_type_name
                })
                .map(|(k, _)| *k);

            if let Some(k) = key {
                edge_types
                    .remove(&k)
                    .map(|d| (d.info.edge_type_id, d.info.edge_type_name))
            } else {
                None
            }
        };

        if let Some((edge_type_id, type_name)) = removed {
            self.emit_schema_event(SchemaChangeEvent::EdgeTypeDropped {
                space_id: space_info.space_id,
                space_name: space_name.to_string(),
                edge_type_id,
                type_name,
            });
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn get_edge_type(
        &self,
        space_name: &str,
        edge_type_name: &str,
    ) -> Result<Option<EdgeTypeInfo>, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let edge_types = self.edge_types.read();
        Ok(edge_types
            .iter()
            .find(|((sid, _), data)| {
                *sid == space_info.space_id && data.info.edge_type_name == edge_type_name
            })
            .map(|(_, data)| data)
            .map(|d| d.info.clone()))
    }

    pub fn list_edge_types(&self, space_name: &str) -> Result<Vec<EdgeTypeInfo>, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let edge_types = self.edge_types.read();
        Ok(edge_types
            .iter()
            .filter(|((sid, _), _)| *sid == space_info.space_id)
            .map(|(_, data)| data.info.clone())
            .collect())
    }

    pub fn find_edge_type_by_id(&self, edge_type_id: u32) -> Option<(String, EdgeTypeInfo)> {
        let edge_types = self.edge_types.read();
        let spaces = self.spaces.read();

        edge_types
            .iter()
            .find_map(|((space_id, current_edge_type_id), data)| {
                if *current_edge_type_id != edge_type_id {
                    return None;
                }

                let space_name = spaces
                    .get(space_id)
                    .map(|space| space.info.space_name.clone())?;

                Some((space_name, data.info.clone()))
            })
    }

    pub fn update_edge_type(
        &self,
        space_name: &str,
        edge_type: &EdgeTypeInfo,
    ) -> Result<bool, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let (applied, edge_type_id) = {
            let mut edge_types = self.edge_types.write();
            let key = edge_types
                .iter()
                .find(|((sid, _), data)| {
                    *sid == space_info.space_id
                        && data.info.edge_type_name == edge_type.edge_type_name
                })
                .map(|(k, _)| *k);

            if let Some(k) = key {
                if let Some(data) = edge_types.get_mut(&k) {
                    let id = data.info.edge_type_id;
                    data.info = edge_type.clone();
                    (true, id)
                } else {
                    (false, 0)
                }
            } else {
                (false, 0)
            }
        };
        if applied {
            self.emit_schema_event(SchemaChangeEvent::EdgeTypeAltered {
                space_id: space_info.space_id,
                space_name: space_name.to_string(),
                edge_type_id,
                type_name: edge_type.edge_type_name.clone(),
                added_properties: edge_type
                    .properties
                    .iter()
                    .map(|p| p.name.clone())
                    .collect(),
                removed_properties: Vec::new(),
            });
        }
        Ok(applied)
    }

    pub fn alter_tag(
        &self,
        space_name: &str,
        tag_name: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> Result<bool, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let applied: Option<(u32, Vec<String>, Vec<String>)> = {
            let mut tags = self.tags.write();
            let tag_key = tags
                .iter()
                .find(|((sid, _), data)| {
                    *sid == space_info.space_id && data.info.tag_name == tag_name
                })
                .map(|(k, _)| *k);

            if let Some(key) = tag_key {
                if let Some(data) = tags.get_mut(&key) {
                    let added: Vec<String> = additions.iter().map(|p| p.name.clone()).collect();
                    for prop in additions {
                        if !data.info.properties.iter().any(|p| p.name == prop.name) {
                            data.info.properties.push(prop);
                        }
                    }
                    data.info
                        .properties
                        .retain(|p| !deletions.contains(&p.name));
                    Some((data.info.tag_id, added, deletions))
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some((tag_id, added, removed)) = applied {
            self.emit_schema_event(SchemaChangeEvent::TagAltered {
                space_id: space_info.space_id,
                space_name: space_name.to_string(),
                tag_id,
                tag_name: tag_name.to_string(),
                added_properties: added,
                removed_properties: removed,
            });
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub fn rename_tag_property(
        &self,
        space_name: &str,
        tag_name: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<bool, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let applied: Option<(u32, Vec<String>, Vec<String>)> = {
            let mut tags = self.tags.write();
            let tag_key = tags
                .iter()
                .find(|((sid, _), data)| {
                    *sid == space_info.space_id && data.info.tag_name == tag_name
                })
                .map(|(k, _)| *k);

            if let Some(key) = tag_key {
                if let Some(data) = tags.get_mut(&key) {
                    if let Some(prop) = data.info.properties.iter_mut().find(|p| p.name == old_name)
                    {
                        prop.name = new_name.to_string();
                        Some((
                            data.info.tag_id,
                            vec![new_name.to_string()],
                            vec![old_name.to_string()],
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some((tag_id, added, removed)) = applied {
            self.emit_schema_event(SchemaChangeEvent::TagAltered {
                space_id: space_info.space_id,
                space_name: space_name.to_string(),
                tag_id,
                tag_name: tag_name.to_string(),
                added_properties: added,
                removed_properties: removed,
            });
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn rename_tag(
        &self,
        space_name: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<bool, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let mut tags = self.tags.write();

        let tag_key = tags
            .iter()
            .find(|((sid, _), data)| *sid == space_info.space_id && data.info.tag_name == old_name)
            .map(|(k, _)| *k);

        if let Some(old_key) = tag_key {
            let data = tags
                .remove(&old_key)
                .ok_or_else(|| StorageError::db_error(format!("Tag \"{}\" not found", old_name)))?;

            if tags
                .iter()
                .any(|((sid, _), d)| *sid == space_info.space_id && d.info.tag_name == new_name)
            {
                tags.insert(old_key, data);
                return Err(StorageError::db_error(format!(
                    "Tag \"{}\" already exists",
                    new_name
                )));
            }

            let mut new_data = data;
            new_data.info.tag_name = new_name.to_string();
            let renamed_tag_id = new_data.info.tag_id;

            let new_key = (old_key.0, old_key.1);
            tags.insert(new_key, new_data);
            drop(tags);

            {
                let mut edge_types = self.edge_types.write();
                for ((sid, _), data) in edge_types.iter_mut() {
                    if *sid == space_info.space_id {
                        if data.info.src_tag_name == old_name {
                            data.info.src_tag_name = new_name.to_string();
                        }
                        if data.info.dst_tag_name == old_name {
                            data.info.dst_tag_name = new_name.to_string();
                        }
                    }
                }
            }

            self.emit_schema_event(SchemaChangeEvent::TagRenamed {
                space_id: space_info.space_id,
                space_name: space_name.to_string(),
                tag_id: renamed_tag_id,
                old_name: old_name.to_string(),
                new_name: new_name.to_string(),
            });

            return Ok(true);
        }

        Ok(false)
    }

    pub fn rename_edge_type(
        &self,
        space_name: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<bool, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let renamed: Option<u32> = {
            let mut edge_types = self.edge_types.write();

            let key = edge_types
                .iter()
                .find(|((sid, _), data)| {
                    *sid == space_info.space_id && data.info.edge_type_name == old_name
                })
                .map(|(k, _)| *k);

            if let Some(old_key) = key {
                let data = edge_types.remove(&old_key).ok_or_else(|| {
                    StorageError::db_error(format!("Edge type \"{}\" not found", old_name))
                })?;

                if edge_types.iter().any(|((sid, _), d)| {
                    *sid == space_info.space_id && d.info.edge_type_name == new_name
                }) {
                    edge_types.insert(old_key, data);
                    return Err(StorageError::db_error(format!(
                        "Edge type \"{}\" already exists",
                        new_name
                    )));
                }

                let mut new_data = data;
                new_data.info.edge_type_name = new_name.to_string();
                let renamed_id = new_data.info.edge_type_id;

                let new_key = (old_key.0, old_key.1);
                edge_types.insert(new_key, new_data);

                Some(renamed_id)
            } else {
                None
            }
        };

        if let Some(edge_type_id) = renamed {
            self.emit_schema_event(SchemaChangeEvent::EdgeTypeRenamed {
                space_id: space_info.space_id,
                space_name: space_name.to_string(),
                edge_type_id,
                old_name: old_name.to_string(),
                new_name: new_name.to_string(),
            });
            return Ok(true);
        }

        Ok(false)
    }

    pub fn alter_edge_type(
        &self,
        space_name: &str,
        edge_type_name: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> Result<bool, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let applied: Option<(u32, Vec<String>, Vec<String>)> = {
            let mut edge_types = self.edge_types.write();
            let key = edge_types
                .iter()
                .find(|((sid, _), data)| {
                    *sid == space_info.space_id && data.info.edge_type_name == edge_type_name
                })
                .map(|(k, _)| *k);

            if let Some(k) = key {
                if let Some(data) = edge_types.get_mut(&k) {
                    let added: Vec<String> = additions.iter().map(|p| p.name.clone()).collect();
                    for prop in additions {
                        if !data.info.properties.iter().any(|p| p.name == prop.name) {
                            data.info.properties.push(prop);
                        }
                    }
                    data.info
                        .properties
                        .retain(|p| !deletions.contains(&p.name));
                    Some((data.info.edge_type_id, added, deletions))
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some((edge_type_id, added, removed)) = applied {
            self.emit_schema_event(SchemaChangeEvent::EdgeTypeAltered {
                space_id: space_info.space_id,
                space_name: space_name.to_string(),
                edge_type_id,
                type_name: edge_type_name.to_string(),
                added_properties: added,
                removed_properties: removed,
            });
            return Ok(true);
        }
        Ok(false)
    }

    pub fn save_schema(&self, path: &Path) -> Result<(), StorageError> {
        use std::fs::{self, File};
        use std::io::Write;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| StorageError::io_error(e.to_string()))?;
        }

        let spaces: Vec<SpaceInfo> = self
            .spaces
            .read()
            .values()
            .map(|d| d.info.clone())
            .collect();

        let tags: Vec<(u64, TagInfo)> = self
            .tags
            .read()
            .iter()
            .map(|((space_id, _), data)| (*space_id, data.info.clone()))
            .collect();

        let edge_types: Vec<(u64, EdgeTypeInfo)> = self
            .edge_types
            .read()
            .iter()
            .map(|((space_id, _), data)| (*space_id, data.info.clone()))
            .collect();

        let space_id_counter = self
            .space_id_counter
            .load(std::sync::atomic::Ordering::SeqCst);

        let tag_id_counters: Vec<(u64, u32)> = self
            .tag_id_counter
            .iter()
            .map(|entry| (*entry.key(), entry.value().load(Ordering::SeqCst)))
            .collect();

        let edge_type_id_counters: Vec<(u64, u32)> = self
            .edge_type_id_counter
            .iter()
            .map(|entry| (*entry.key(), entry.value().load(Ordering::SeqCst)))
            .collect();

        let snapshot = SchemaSnapshot {
            version: SCHEMA_FORMAT_VERSION,
            spaces,
            tags,
            edge_types,
            space_id_counter,
            tag_id_counters,
            edge_type_id_counters,
            serial_next: self.serial_next.read().clone(),
            sequences: self.sequences.read().values().cloned().collect(),
        };

        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;

        let mut file = File::create(path).map_err(|e| StorageError::io_error(e.to_string()))?;

        file.write_all(json.as_bytes())
            .map_err(|e| StorageError::io_error(e.to_string()))?;

        Ok(())
    }

    pub fn load_schema(&self, path: &Path) -> Result<(), StorageError> {
        use std::fs::File;
        use std::io::Read;

        if !path.exists() {
            return Ok(());
        }

        let mut file = File::open(path).map_err(|e| StorageError::io_error(e.to_string()))?;

        let mut json = String::new();
        file.read_to_string(&mut json)
            .map_err(|e| StorageError::io_error(e.to_string()))?;

        let snapshot: SchemaSnapshot = serde_json::from_str(&json)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;

        if snapshot.version > SCHEMA_FORMAT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "Schema version {} is newer than supported version {}",
                snapshot.version, SCHEMA_FORMAT_VERSION
            )));
        }

        self.spaces.write().clear();
        self.space_name_index.write().clear();
        self.tags.write().clear();
        self.edge_types.write().clear();
        self.tag_id_counter.clear();
        self.edge_type_id_counter.clear();

        let max_tag_counter = snapshot
            .tag_id_counters
            .iter()
            .map(|(_, counter)| *counter)
            .max()
            .unwrap_or(0)
            .max(
                snapshot
                    .tags
                    .iter()
                    .map(|(_, tag)| tag.tag_id)
                    .max()
                    .unwrap_or(0),
            );
        let max_edge_type_counter = snapshot
            .edge_type_id_counters
            .iter()
            .map(|(_, counter)| *counter)
            .max()
            .unwrap_or(0)
            .max(
                snapshot
                    .edge_types
                    .iter()
                    .map(|(_, edge_type)| edge_type.edge_type_id)
                    .max()
                    .unwrap_or(0),
            );

        for space in snapshot.spaces {
            self.space_name_index
                .write()
                .insert(space.space_name.clone(), space.space_id);
            self.spaces
                .write()
                .insert(space.space_id, SpaceData { info: space });
        }

        for (space_id, tag) in snapshot.tags {
            self.tags
                .write()
                .insert((space_id, tag.tag_id), TagData { info: tag });
        }

        for (space_id, edge_type) in snapshot.edge_types {
            self.edge_types.write().insert(
                (space_id, edge_type.edge_type_id),
                EdgeTypeData { info: edge_type },
            );
        }

        self.space_id_counter
            .store(snapshot.space_id_counter, Ordering::SeqCst);

        for (space_id, counter) in snapshot.tag_id_counters {
            self.tag_id_counter
                .insert(space_id, AtomicU32::new(counter));
        }
        self.tag_id_counter
            .insert(0, AtomicU32::new(max_tag_counter));

        for (space_id, counter) in snapshot.edge_type_id_counters {
            self.edge_type_id_counter
                .insert(space_id, AtomicU32::new(counter));
        }
        self.edge_type_id_counter
            .insert(0, AtomicU32::new(max_edge_type_counter));

        *self.serial_next.write() = snapshot.serial_next;

        let mut sequences = self.sequences.write();
        sequences.clear();
        for def in snapshot.sequences {
            sequences.insert(def.name.clone(), def);
        }

        Ok(())
    }

    // ==================== Sequence Operations ====================

    /// Create a sequence
    pub fn create_sequence(
        &self,
        name: String,
        start: i64,
        increment: i64,
        min_value: i64,
        max_value: i64,
        cycle: bool,
    ) -> Result<(), StorageError> {
        {
            let mut sequences = self.sequences.write();
            if sequences.contains_key(&name) {
                return Err(StorageError::db_error(format!(
                    "Sequence '{}' already exists",
                    name
                )));
            }
            let def = SequenceDef::new(name.clone(), start, increment, min_value, max_value, cycle);
            sequences.insert(name.clone(), def);
        }
        self.emit_schema_event(SchemaChangeEvent::SequenceCreated { name });
        Ok(())
    }

    /// Drop a sequence
    pub fn drop_sequence(&self, name: &str) -> Result<bool, StorageError> {
        let removed = {
            let mut sequences = self.sequences.write();
            sequences.remove(name).is_some()
        };
        if removed {
            self.emit_schema_event(SchemaChangeEvent::SequenceDropped {
                name: name.to_string(),
            });
        }
        Ok(removed)
    }

    /// Get a sequence definition
    pub fn get_sequence(&self, name: &str) -> Option<SequenceDef> {
        let sequences = self.sequences.read();
        sequences.get(name).cloned()
    }

    /// Get the current value of a sequence
    pub fn sequence_current_value(&self, name: &str) -> Result<i64, StorageError> {
        let sequences = self.sequences.read();
        let def = sequences
            .get(name)
            .ok_or_else(|| StorageError::db_error(format!("Sequence '{}' does not exist", name)))?;
        Ok(def.current_value())
    }

    /// Get the next value of a sequence (atomic increment)
    pub fn sequence_next_value(&self, name: &str) -> Result<i64, StorageError> {
        let sequences = self.sequences.read();
        let def = sequences
            .get(name)
            .ok_or_else(|| StorageError::db_error(format!("Sequence '{}' does not exist", name)))?;
        def.next_value()
    }

    /// Alter sequence properties
    pub fn alter_sequence(
        &self,
        name: &str,
        increment: Option<i64>,
        min_value: Option<i64>,
        max_value: Option<i64>,
        cycle: Option<bool>,
    ) -> Result<(), StorageError> {
        {
            let mut sequences = self.sequences.write();
            let def = sequences.get(name).ok_or_else(|| {
                StorageError::db_error(format!("Sequence '{}' does not exist", name))
            })?;

            let new_increment = increment.unwrap_or(def.increment);
            let new_min = min_value.unwrap_or(def.min_value);
            let new_max = max_value.unwrap_or(def.max_value);
            let new_cycle = cycle.unwrap_or(def.cycle);
            let current = def.current_value();

            let new_def = SequenceDef::new(
                name.to_string(),
                current,
                new_increment,
                new_min,
                new_max,
                new_cycle,
            );
            sequences.insert(name.to_string(), new_def);
        }
        self.emit_schema_event(SchemaChangeEvent::SequenceAltered {
            name: name.to_string(),
        });
        Ok(())
    }

    /// List all sequence names
    pub fn list_sequences(&self) -> Vec<String> {
        let sequences = self.sequences.read();
        sequences.keys().cloned().collect()
    }
}

impl Default for SchemaManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EdgeTypeInfo, SpaceInfo, TagInfo};

    #[test]
    fn schema_names_are_scoped_by_space() {
        let manager = SchemaManager::new();
        let mut first = SpaceInfo::new("first".to_string());
        let mut second = SpaceInfo::new("second".to_string());
        manager
            .create_space(&mut first)
            .expect("create first space");
        manager
            .create_space(&mut second)
            .expect("create second space");

        let first_tag_id = manager
            .create_tag("first", &TagInfo::new("person".to_string()))
            .expect("create first tag");
        let second_tag_id = manager
            .create_tag("second", &TagInfo::new("person".to_string()))
            .expect("create second tag");

        assert_ne!(first_tag_id, second_tag_id);
        assert_eq!(
            manager
                .get_tag("first", "person")
                .expect("get first tag")
                .expect("first tag exists")
                .tag_id,
            first_tag_id
        );
        assert_eq!(
            manager
                .get_tag("second", "person")
                .expect("get second tag")
                .expect("second tag exists")
                .tag_id,
            second_tag_id
        );

        let first_edge_id = manager
            .create_edge_type("first", &EdgeTypeInfo::new("knows".to_string()))
            .expect("create first edge");
        let second_edge_id = manager
            .create_edge_type("second", &EdgeTypeInfo::new("knows".to_string()))
            .expect("create second edge");

        assert_ne!(first_edge_id, second_edge_id);
        assert_eq!(
            manager
                .get_edge_type("first", "knows")
                .expect("get first edge")
                .expect("first edge exists")
                .edge_type_id,
            first_edge_id
        );
        assert_eq!(
            manager
                .get_edge_type("second", "knows")
                .expect("get second edge")
                .expect("second edge exists")
                .edge_type_id,
            second_edge_id
        );
    }
}
