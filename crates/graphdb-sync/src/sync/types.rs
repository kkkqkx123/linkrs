use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeType {
    Insert,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexOpKey {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
}

impl IndexOpKey {
    pub fn new(space_id: u64, tag_name: impl Into<String>, field_name: impl Into<String>) -> Self {
        Self {
            space_id,
            tag_name: tag_name.into(),
            field_name: field_name.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexType {
    Fulltext,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexData {
    Fulltext(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexOperation {
    pub key: IndexOpKey,
    pub index_type: IndexType,
    pub change_type: ChangeType,
    pub id: String,
    pub data: Option<IndexData>,
}

impl IndexOperation {
    pub fn new_fulltext(
        key: IndexOpKey,
        change_type: ChangeType,
        id: impl Into<String>,
        text: Option<String>,
    ) -> Self {
        Self {
            key,
            index_type: IndexType::Fulltext,
            change_type,
            id: id.into(),
            data: text.map(IndexData::Fulltext),
        }
    }

    pub fn extract_index_key(&self) -> (u64, String, String) {
        (
            self.key.space_id,
            self.key.tag_name.clone(),
            self.key.field_name.clone(),
        )
    }

    pub fn space_id(&self) -> u64 {
        self.key.space_id
    }

    pub fn tag_name(&self) -> &str {
        &self.key.tag_name
    }

    pub fn field_name(&self) -> &str {
        &self.key.field_name
    }

    pub fn text(&self) -> Option<&str> {
        match &self.data {
            Some(IndexData::Fulltext(text)) => Some(text),
            None => None,
        }
    }
}

#[deprecated(note = "Use IndexOperation::new_fulltext() instead")]
impl IndexOperation {
    pub fn insert(key: IndexOpKey, id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new_fulltext(key, ChangeType::Insert, id, Some(text.into()))
    }

    pub fn update(key: IndexOpKey, id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new_fulltext(key, ChangeType::Update, id, Some(text.into()))
    }

    pub fn delete(key: IndexOpKey, id: impl Into<String>) -> Self {
        Self::new_fulltext(key, ChangeType::Delete, id, None)
    }
}
