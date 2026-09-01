pub mod data;
pub mod deferred;
pub mod index;
pub mod schema;

use crate::engine::graph_storage::GraphStorageContext;
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::wal::traits::RecoveryApplier;
use graphdb_core::{StorageResult, Value};
use graphdb_transaction::wal::{
    AddEdgePropRedo, AddVertexPropRedo, AlterSpaceCommentRedo, ClearSpaceRedo, CreateEdgeIndexRedo,
    CreateEdgeTypeRedo, CreateSpaceRedo, CreateTagIndexRedo, CreateVertexTypeRedo,
    DeleteEdgePropRedo, DeleteEdgeRedo, DeleteEdgeTypeRedo, DeleteVertexPropRedo,
    DeleteVertexTypeRedo, DropEdgeIndexRedo, DropSpaceRedo, DropTagIndexRedo, InsertEdgeRedo,
    RenameEdgePropRedo, RenameVertexPropRedo, UpdateEdgePropRedo, UpdateSequenceRedo,
};

impl RecoveryApplier for GraphStorageContext {
    fn replay_insert_vertex(
        &self,
        label: LabelId,
        vid: VertexId,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        data::replay_insert_vertex(self, label, vid, properties, ts)
    }

    fn replay_insert_edge(&self, redo: &InsertEdgeRedo, ts: Timestamp) -> StorageResult<()> {
        data::replay_insert_edge(self, redo, ts)
    }

    fn replay_delete_edge(&self, redo: &DeleteEdgeRedo, ts: Timestamp) -> StorageResult<()> {
        data::replay_delete_edge(self, redo, ts)
    }

    fn replay_update_vertex_prop(
        &self,
        label: LabelId,
        vid: VertexId,
        prop_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        data::replay_update_vertex_prop(self, label, vid, prop_name, value, ts)
    }

    fn replay_update_edge_prop(
        &self,
        redo: &UpdateEdgePropRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        data::replay_update_edge_prop(self, redo, ts)
    }

    fn replay_delete_vertex(
        &self,
        label: LabelId,
        vid: VertexId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        data::replay_delete_vertex(self, label, vid, ts)
    }

    fn replay_create_space(&self, redo: &CreateSpaceRedo, ts: Timestamp) -> StorageResult<()> {
        schema::replay_create_space(self, redo, ts)
    }

    fn replay_drop_space(&self, redo: &DropSpaceRedo, ts: Timestamp) -> StorageResult<()> {
        schema::replay_drop_space(self, redo, ts)
    }

    fn replay_clear_space(&self, redo: &ClearSpaceRedo, ts: Timestamp) -> StorageResult<()> {
        schema::replay_clear_space(self, redo, ts)
    }

    fn replay_alter_space_comment(
        &self,
        redo: &AlterSpaceCommentRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        schema::replay_alter_space_comment(self, redo, ts)
    }

    fn replay_create_vertex_type(
        &self,
        redo: &CreateVertexTypeRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        schema::replay_create_vertex_type(self, redo, ts)
    }

    fn replay_create_edge_type(
        &self,
        redo: &CreateEdgeTypeRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        schema::replay_create_edge_type(self, redo, ts)
    }

    fn replay_delete_vertex_type(
        &self,
        redo: &DeleteVertexTypeRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        schema::replay_delete_vertex_type(self, redo, ts)
    }

    fn replay_delete_edge_type(
        &self,
        redo: &DeleteEdgeTypeRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        schema::replay_delete_edge_type(self, redo, ts)
    }

    fn replay_add_vertex_prop(&self, redo: &AddVertexPropRedo, ts: Timestamp) -> StorageResult<()> {
        schema::replay_add_vertex_prop(self, redo, ts)
    }

    fn replay_add_edge_prop(&self, redo: &AddEdgePropRedo, ts: Timestamp) -> StorageResult<()> {
        schema::replay_add_edge_prop(self, redo, ts)
    }

    fn replay_delete_vertex_prop(
        &self,
        redo: &DeleteVertexPropRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        schema::replay_delete_vertex_prop(self, redo, ts)
    }

    fn replay_delete_edge_prop(
        &self,
        redo: &DeleteEdgePropRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        schema::replay_delete_edge_prop(self, redo, ts)
    }

    fn replay_rename_vertex_prop(
        &self,
        redo: &RenameVertexPropRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        schema::replay_rename_vertex_prop(self, redo, ts)
    }

    fn replay_rename_edge_prop(
        &self,
        redo: &RenameEdgePropRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        schema::replay_rename_edge_prop(self, redo, ts)
    }

    fn replay_create_tag_index(
        &self,
        redo: &CreateTagIndexRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        index::replay_create_tag_index(self, redo, ts)
    }

    fn replay_drop_tag_index(&self, redo: &DropTagIndexRedo, ts: Timestamp) -> StorageResult<()> {
        index::replay_drop_tag_index(self, redo, ts)
    }

    fn replay_create_edge_index(
        &self,
        redo: &CreateEdgeIndexRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        index::replay_create_edge_index(self, redo, ts)
    }

    fn replay_drop_edge_index(&self, redo: &DropEdgeIndexRedo, ts: Timestamp) -> StorageResult<()> {
        index::replay_drop_edge_index(self, redo, ts)
    }

    fn replay_update_sequence(&self, redo: &UpdateSequenceRedo, _ts: Timestamp) -> StorageResult<()> {
        self.serial_allocator()
            .seed(&crate::engine::graph_storage::SerialKey::new(redo.space_id, &redo.table_name), redo.next_value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::graph_storage::GraphStorageContext;
    use crate::engine::{EdgeOperationParams, InsertEdgeParams};
    use graphdb_core::types::VertexId;
    use graphdb_core::wal::traits::RecoveryApplier;
    use graphdb_core::Value;
    use graphdb_transaction::wal::{
        AddEdgePropRedo, AddVertexPropRedo, CreateEdgeTypeRedo, CreateVertexTypeRedo,
        DeleteEdgePropRedo, DeleteVertexPropRedo, RenameEdgePropRedo, RenameVertexPropRedo,
    };

    #[test]
    fn test_schema_replay_roundtrip() {
        let ctx = GraphStorageContext::new();

        ctx.replay_create_vertex_type(
            &CreateVertexTypeRedo {
                space_name: "test_space".to_string(),
                label_id: Some(1),
                label_name: "Person".to_string(),
                schema: vec![
                    ("id".to_string(), "BIGINT".to_string(), true),
                    ("name".to_string(), "STRING".to_string(), false),
                ],
            },
            1,
        )
        .expect("Vertex type replay should succeed");

        ctx.replay_create_vertex_type(
            &CreateVertexTypeRedo {
                space_name: "test_space".to_string(),
                label_id: Some(2),
                label_name: "City".to_string(),
                schema: vec![
                    ("id".to_string(), "BIGINT".to_string(), false),
                    ("name".to_string(), "STRING".to_string(), false),
                ],
            },
            1,
        )
        .expect("Second vertex type replay should succeed");

        let person_label = ctx
            .data_store()
            .vertex_label_id("space_1:tag:Person")
            .expect("Person label should exist");
        let city_label = ctx
            .data_store()
            .vertex_label_id("space_1:tag:City")
            .expect("City label should exist");

        ctx.replay_add_vertex_prop(
            &AddVertexPropRedo {
                label: person_label,
                properties: vec![("age".to_string(), "INT".to_string(), false)],
            },
            2,
        )
        .expect("Vertex property replay should succeed");

        ctx.replay_rename_vertex_prop(
            &RenameVertexPropRedo {
                label: person_label,
                old_name: "name".to_string(),
                new_name: "full_name".to_string(),
            },
            2,
        )
        .expect("Vertex rename replay should succeed");

        ctx.replay_delete_vertex_prop(
            &DeleteVertexPropRedo {
                label: person_label,
                prop_names: vec!["age".to_string()],
            },
            2,
        )
        .expect("Vertex delete replay should succeed");

        ctx.replay_create_edge_type(
            &CreateEdgeTypeRedo {
                space_name: "test_space".to_string(),
                label_id: Some(3),
                src_label: "Person".to_string(),
                dst_label: "City".to_string(),
                edge_label: "LIVES_IN".to_string(),
                schema: vec![("since".to_string(), "INT".to_string(), false)],
            },
            3,
        )
        .expect("Edge type replay should succeed");

        let lives_in_label = 3;

        ctx.replay_add_edge_prop(
            &AddEdgePropRedo {
                src_label: person_label,
                dst_label: city_label,
                edge_label: lives_in_label,
                properties: vec![("cost".to_string(), "INT".to_string(), false)],
            },
            3,
        )
        .expect("Edge property replay should succeed");

        ctx.replay_rename_edge_prop(
            &RenameEdgePropRedo {
                src_label: person_label,
                dst_label: city_label,
                edge_label: lives_in_label,
                old_name: "since".to_string(),
                new_name: "started".to_string(),
            },
            3,
        )
        .expect("Edge rename replay should succeed");

        ctx.replay_delete_edge_prop(
            &DeleteEdgePropRedo {
                src_label: person_label,
                dst_label: city_label,
                edge_label: lives_in_label,
                prop_names: vec!["cost".to_string()],
            },
            3,
        )
        .expect("Edge delete replay should succeed");

        let person_tag = ctx
            .schema_manager()
            .find_tag_by_id(person_label)
            .expect("Person tag should exist")
            .1;
        assert_eq!(
            person_tag
                .properties
                .iter()
                .map(|prop| prop.name.as_str())
                .collect::<Vec<_>>(),
            vec!["id", "full_name"]
        );

        let lives_in_type = ctx
            .schema_manager()
            .find_edge_type_by_id(lives_in_label)
            .expect("Edge type should exist")
            .1;
        assert_eq!(
            lives_in_type
                .properties
                .iter()
                .map(|prop| prop.name.as_str())
                .collect::<Vec<_>>(),
            vec!["started"]
        );

        ctx.insert_vertex_by_i64(
            person_label,
            1001,
            &[
                ("id".to_string(), Value::BigInt(1001)),
                ("full_name".to_string(), Value::string("Alice")),
            ],
            4,
        )
        .expect("Vertex insert should succeed after property replay");

        ctx.insert_vertex_by_i64(
            city_label,
            2001,
            &[
                ("id".to_string(), Value::BigInt(2001)),
                ("name".to_string(), Value::string("Shanghai")),
            ],
            4,
        )
        .expect("City vertex insert should succeed");

        let vertex = ctx
            .get_vertex_by_i64(person_label, 1001, 5)
            .expect("Inserted vertex should be visible");
        assert_eq!(
            vertex
                .properties
                .iter()
                .find(|(name, _)| name == "full_name")
                .map(|(_, value)| value),
            Some(&Value::string("Alice"))
        );
        assert!(vertex.properties.iter().all(|(name, _)| name != "age"));

        ctx.insert_edge(InsertEdgeParams {
            edge_label: lives_in_label,
            src_label: person_label,
            src_id: VertexId::from_int64(1001),
            dst_label: city_label,
            dst_id: VertexId::from_int64(2001),
            rank: 0,
            properties: &[("started".to_string(), Value::Int(2012))],
            ts: 5,
        })
        .expect("Edge insert should succeed after property replay");

        let edge = ctx
            .get_edge(
                &EdgeOperationParams {
                    edge_label: lives_in_label,
                    src_label: person_label,
                    src_id: VertexId::from_int64(1001),
                    dst_label: city_label,
                    dst_id: VertexId::from_int64(2001),
                    rank: 0,
                },
                5,
            )
            .expect("Inserted edge should be visible");
        assert_eq!(
            edge.properties
                .iter()
                .find(|(name, _)| name == "started")
                .map(|(_, value)| value),
            Some(&Value::Int(2012))
        );
        assert!(edge.properties.iter().all(|(name, _)| name != "cost"));
        assert!(edge.properties.iter().all(|(name, _)| name != "since"));
    }
}
