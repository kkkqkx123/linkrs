use crate::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo};
use crate::core::StorageError;
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
        }
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

        Ok(true)
    }

    pub fn drop_space(&self, space_name: &str) -> Result<bool, StorageError> {
        let mut name_index = self.space_name_index.write();
        if let Some(space_id) = name_index.remove(space_name) {
            drop(name_index);

            let mut spaces = self.spaces.write();
            spaces.remove(&space_id);

            let mut tags = self.tags.write();
            tags.retain(|(sid, _), _| *sid != space_id);

            let mut edge_types = self.edge_types.write();
            edge_types.retain(|(sid, _), _| *sid != space_id);

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

        let mut edge_types = self.edge_types.write();
        edge_types.retain(|(sid, _), _| *sid != space_id);

        Ok(true)
    }

    pub fn alter_space_comment(
        &self,
        space_id: u64,
        comment: String,
    ) -> Result<bool, StorageError> {
        let mut spaces = self.spaces.write();
        if let Some(data) = spaces.get_mut(&space_id) {
            data.info.comment = Some(comment);
            Ok(true)
        } else {
            Ok(false)
        }
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
        let mut spaces = self.spaces.write();
        if let std::collections::hash_map::Entry::Occupied(mut e) = spaces.entry(space.space_id) {
            e.insert(SpaceData {
                info: space.clone(),
            });
            Ok(true)
        } else {
            Ok(false)
        }
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

        let mut tags = self.tags.write();
        tags.insert((space_info.space_id, tag_id), TagData { info: tag_with_id });

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

        let mut tags = self.tags.write();
        tags.insert((space_info.space_id, tag_id), TagData { info: tag_with_id });

        let entry = self
            .tag_id_counter
            .entry(0)
            .or_insert_with(|| AtomicU32::new(0));
        let current = entry.load(Ordering::SeqCst);
        if tag_id > current {
            entry.store(tag_id, Ordering::SeqCst);
        }

        Ok(tag_id)
    }

    pub fn drop_tag(&self, space_name: &str, tag_name: &str) -> Result<bool, StorageError> {
        let space_info = self.get_space(space_name)?.ok_or_else(|| {
            StorageError::db_error(format!("Space \"{}\" does not exist", space_name))
        })?;

        let mut tags = self.tags.write();
        let tag_key = tags
            .iter()
            .find(|((sid, _), data)| *sid == space_info.space_id && data.info.tag_name == tag_name)
            .map(|(k, _)| *k);

        if let Some(key) = tag_key {
            tags.remove(&key);
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

        let mut tags = self.tags.write();
        let tag_key = tags
            .iter()
            .find(|((sid, _), data)| {
                *sid == space_info.space_id && data.info.tag_name == tag.tag_name
            })
            .map(|(k, _)| *k);

        if let Some(key) = tag_key {
            if let Some(data) = tags.get_mut(&key) {
                data.info = tag.clone();
                return Ok(true);
            }
        }
        Ok(false)
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

        let mut edge_types = self.edge_types.write();
        edge_types.insert(
            (space_info.space_id, edge_type_id),
            EdgeTypeData { info: edge_with_id },
        );

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

        let mut edge_types = self.edge_types.write();
        edge_types.insert(
            (space_info.space_id, edge_type_id),
            EdgeTypeData { info: edge_with_id },
        );

        let entry = self
            .edge_type_id_counter
            .entry(0)
            .or_insert_with(|| AtomicU32::new(0));
        let current = entry.load(Ordering::SeqCst);
        if edge_type_id > current {
            entry.store(edge_type_id, Ordering::SeqCst);
        }

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

        let mut edge_types = self.edge_types.write();
        let key = edge_types
            .iter()
            .find(|((sid, _), data)| {
                *sid == space_info.space_id && data.info.edge_type_name == edge_type_name
            })
            .map(|(k, _)| *k);

        if let Some(k) = key {
            edge_types.remove(&k);
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

        let mut edge_types = self.edge_types.write();
        let key = edge_types
            .iter()
            .find(|((sid, _), data)| {
                *sid == space_info.space_id && data.info.edge_type_name == edge_type.edge_type_name
            })
            .map(|(k, _)| *k);

        if let Some(k) = key {
            if let Some(data) = edge_types.get_mut(&k) {
                data.info = edge_type.clone();
                return Ok(true);
            }
        }
        Ok(false)
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

        let mut tags = self.tags.write();
        let tag_key = tags
            .iter()
            .find(|((sid, _), data)| *sid == space_info.space_id && data.info.tag_name == tag_name)
            .map(|(k, _)| *k);

        if let Some(key) = tag_key {
            if let Some(data) = tags.get_mut(&key) {
                for prop in additions {
                    if !data.info.properties.iter().any(|p| p.name == prop.name) {
                        data.info.properties.push(prop);
                    }
                }
                data.info
                    .properties
                    .retain(|p| !deletions.contains(&p.name));
                return Ok(true);
            }
        }
        Ok(false)
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

        let mut tags = self.tags.write();
        let tag_key = tags
            .iter()
            .find(|((sid, _), data)| *sid == space_info.space_id && data.info.tag_name == tag_name)
            .map(|(k, _)| *k);

        if let Some(key) = tag_key {
            if let Some(data) = tags.get_mut(&key) {
                if let Some(prop) = data.info.properties.iter_mut().find(|p| p.name == old_name) {
                    prop.name = new_name.to_string();
                    return Ok(true);
                }
            }
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

        let mut edge_types = self.edge_types.write();
        let key = edge_types
            .iter()
            .find(|((sid, _), data)| {
                *sid == space_info.space_id && data.info.edge_type_name == edge_type_name
            })
            .map(|(k, _)| *k);

        if let Some(k) = key {
            if let Some(data) = edge_types.get_mut(&k) {
                for prop in additions {
                    if !data.info.properties.iter().any(|p| p.name == prop.name) {
                        data.info.properties.push(prop);
                    }
                }
                data.info
                    .properties
                    .retain(|p| !deletions.contains(&p.name));
                return Ok(true);
            }
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

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{EdgeTypeInfo, SpaceInfo, TagInfo};

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

impl Default for SchemaManager {
    fn default() -> Self {
        Self::new()
    }
}
