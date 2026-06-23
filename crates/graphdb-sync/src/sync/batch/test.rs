#[cfg(test)]
mod buffer_tests {
    use super::super::buffer::OpBatchBuffer;
    use crate::sync::types::{ChangeType, IndexOpKey, IndexOperation};

    fn test_key() -> IndexOpKey {
        IndexOpKey::new(1, "tag", "field")
    }

    fn test_key_tuple() -> (u64, String, String) {
        (1, "tag".to_string(), "field".to_string())
    }

    fn insert_op(id: &str, text: &str) -> IndexOperation {
        IndexOperation::new_fulltext(test_key(), ChangeType::Insert, id, Some(text.to_string()))
    }

    #[test]
    fn test_batch_buffer_add_and_count() {
        let buffer = OpBatchBuffer::new();
        let key = test_key_tuple();

        buffer.add_insert(&key, insert_op("1", "text1"));
        buffer.add_insert(&key, insert_op("2", "text2"));
        buffer.add_delete(&key, "3".to_string());

        assert_eq!(buffer.count(&key), 3);
        assert_eq!(buffer.insert_count(&key), 2);
        assert_eq!(buffer.delete_count(&key), 1);
        assert_eq!(buffer.total_count(), 3);
    }

    #[test]
    fn test_batch_buffer_drain() {
        let buffer = OpBatchBuffer::new();
        let key = test_key_tuple();

        buffer.add_insert(&key, insert_op("1", "text1"));
        buffer.add_insert(&key, insert_op("2", "text2"));
        buffer.add_delete(&key, "3".to_string());

        let inserts = buffer.drain_inserts(&key);
        assert_eq!(inserts.len(), 2);

        let deletes = buffer.drain_deletes(&key);
        assert_eq!(deletes.len(), 1);
        assert_eq!(deletes[0], "3");

        assert_eq!(buffer.count(&key), 0);
    }

    #[test]
    fn test_batch_buffer_clear() {
        let buffer = OpBatchBuffer::new();
        let key = test_key_tuple();

        buffer.add_insert(&key, insert_op("1", "text1"));
        assert_eq!(buffer.total_count(), 1);
        buffer.clear();
        assert_eq!(buffer.total_count(), 0);
    }

    #[test]
    fn test_index_operation_new_fulltext() {
        let key = test_key();
        let op = IndexOperation::new_fulltext(
            key.clone(),
            ChangeType::Insert,
            "1",
            Some("text".to_string()),
        );

        assert_eq!(op.key, key);
        assert_eq!(op.change_type, ChangeType::Insert);
        assert_eq!(op.id, "1");
        assert_eq!(op.text(), Some("text"));
        assert_eq!(op.index_type, crate::sync::types::IndexType::Fulltext);
    }

    #[test]
    fn test_index_operation_delete_has_no_text() {
        let key = test_key();
        let op = IndexOperation::new_fulltext(key, ChangeType::Delete, "1", None);

        assert_eq!(op.change_type, ChangeType::Delete);
        assert_eq!(op.text(), None);
    }

    #[test]
    fn test_index_operation_extract_key() {
        let op = insert_op("1", "text");

        let extracted = op.extract_index_key();
        assert_eq!(extracted, (1, "tag".to_string(), "field".to_string()));
    }

    #[test]
    fn test_change_type_equality() {
        assert_eq!(ChangeType::Insert, ChangeType::Insert);
        assert_ne!(ChangeType::Insert, ChangeType::Update);
        assert_ne!(ChangeType::Update, ChangeType::Delete);
    }

    #[test]
    fn test_index_op_key_creation() {
        let key = IndexOpKey::new(42, "my_tag", "my_field");
        assert_eq!(key.space_id, 42);
        assert_eq!(key.tag_name, "my_tag");
        assert_eq!(key.field_name, "my_field");
    }
}
