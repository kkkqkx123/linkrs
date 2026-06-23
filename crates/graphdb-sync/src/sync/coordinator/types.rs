use serde::{Deserialize, Serialize};

pub use crate::sync::types::ChangeType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexType {
    Fulltext,
}

#[derive(Debug, Clone)]
pub struct ChangeContext {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub index_type: IndexType,
    pub change_type: ChangeType,
    pub vertex_id: String,
    pub data: ChangeData,
}

#[derive(Debug, Clone)]
pub enum ChangeData {
    Fulltext(String),
}

impl ChangeContext {
    pub fn new_fulltext(
        space_id: u64,
        tag_name: impl Into<String>,
        field_name: impl Into<String>,
        change_type: ChangeType,
        vertex_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            space_id,
            tag_name: tag_name.into(),
            field_name: field_name.into(),
            index_type: IndexType::Fulltext,
            change_type,
            vertex_id: vertex_id.into(),
            data: ChangeData::Fulltext(text.into()),
        }
    }

    pub fn index_key(&self) -> (u64, String, String) {
        (
            self.space_id,
            self.tag_name.clone(),
            self.field_name.clone(),
        )
    }
}
