use chrono::{DateTime, Utc};
use linkrs_config::fulltext::FulltextBackendKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadata {
    pub index_id: String,
    pub index_name: String,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    /// Storage backend serving this index. This is the same value the engine
    /// factory dispatches on, so metadata can never disagree with behaviour.
    #[serde(default)]
    pub backend: FulltextBackendKind,
    pub storage_path: String,
    pub created_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub doc_count: usize,
    pub status: IndexStatus,
    pub engine_config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexStatus {
    Creating,
    Active,
    Rebuilding,
    Disabled,
    Error,
}

impl std::fmt::Display for IndexStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexStatus::Creating => write!(f, "CREATING"),
            IndexStatus::Active => write!(f, "ACTIVE"),
            IndexStatus::Rebuilding => write!(f, "REBUILDING"),
            IndexStatus::Disabled => write!(f, "DISABLED"),
            IndexStatus::Error => write!(f, "ERROR"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndexKey {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
}

const FULLTEXT_INDEX_PREFIX: &str = "space_ft";

impl IndexKey {
    pub fn new(space_id: u64, tag_name: &str, field_name: &str) -> Self {
        Self {
            space_id,
            tag_name: tag_name.to_string(),
            field_name: field_name.to_string(),
        }
    }

    /// Physical index directory name.
    ///
    /// Tag and field names are length-prefixed so that a name containing the
    /// `_` separator cannot collide with a different `(tag, field)` pair:
    /// `space_ft_1_8_User_tag_2_id` always decodes to `("User_tag", "id")` and
    /// never to `("User", "tag_2_id")`.
    pub fn to_index_id(&self) -> String {
        format!(
            "{}_{}_{}_{}_{}_{}",
            FULLTEXT_INDEX_PREFIX,
            self.space_id,
            self.tag_name.len(),
            self.tag_name,
            self.field_name.len(),
            self.field_name
        )
    }

    /// Inverse of [`Self::to_index_id`]. Returns `None` for names that are not
    /// length-prefixed index ids (legacy or unrelated directories).
    pub fn from_index_id(index_id: &str) -> Option<Self> {
        let rest = index_id
            .strip_prefix(FULLTEXT_INDEX_PREFIX)?
            .strip_prefix('_')?;
        let (space_id, rest) = rest.split_once('_')?;
        let space_id: u64 = space_id.parse().ok()?;
        let (tag_len, rest) = rest.split_once('_')?;
        let tag_len: usize = tag_len.parse().ok()?;
        let tag_name = rest.get(..tag_len)?;
        let rest = rest.get(tag_len..)?.strip_prefix('_')?;
        let (field_len, rest) = rest.split_once('_')?;
        let field_len: usize = field_len.parse().ok()?;
        if rest.len() != field_len {
            return None;
        }
        Some(Self {
            space_id,
            tag_name: tag_name.to_string(),
            field_name: rest.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::IndexKey;

    #[test]
    fn index_id_roundtrips_through_parser() {
        for (space_id, tag, field) in [
            (1u64, "Article", "content"),
            (7, "User_Profile", "email_address"),
            (42, "a_b", "c"),
            (42, "a", "b_c"),
            (3, "文章", "正文"),
        ] {
            let key = IndexKey::new(space_id, tag, field);
            let index_id = key.to_index_id();
            let parsed = IndexKey::from_index_id(&index_id).expect("index id should parse");
            assert_eq!(parsed, key, "roundtrip failed for {index_id}");
        }
    }

    #[test]
    fn index_id_disambiguates_underscore_names() {
        let left = IndexKey::new(1, "a_b", "c").to_index_id();
        let right = IndexKey::new(1, "a", "b_c").to_index_id();
        assert_ne!(left, right);
        assert_eq!(left, "space_ft_1_3_a_b_1_c");
        assert_eq!(right, "space_ft_1_1_a_3_b_c");
    }

    #[test]
    fn legacy_index_id_is_not_parsed() {
        assert!(IndexKey::from_index_id("space_ft_1_Article_content").is_none());
        assert!(IndexKey::from_index_id("unrelated_dir").is_none());
        // A truncated body must not panic when the byte offset lands inside a
        // multi-byte character.
        assert!(IndexKey::from_index_id("space_ft_1_6_文章_6_正文").is_some());
        assert!(IndexKey::from_index_id("space_ft_1_1_文_1_x").is_none());
    }
}
