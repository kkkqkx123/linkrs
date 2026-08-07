use std::sync::atomic::Ordering;

use crate::core::types::LabelId;
use crate::core::{StorageError, StorageResult};
use crate::storage::edge::{EdgeSchema, EdgeStore, EdgeStrategy};
use crate::storage::engine::params::CreateEdgeTypeParams;
use crate::storage::types::StoragePropertyDef;
use crate::storage::vertex::{ShardedVertexTable, VertexSchema};

use super::context::GraphStorageContext;

pub fn create_vertex_type(
    ctx: &GraphStorageContext,
    name: &str,
    properties: Vec<StoragePropertyDef>,
    primary_key: &str,
) -> StorageResult<LabelId> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    let primary_key_index = properties
        .iter()
        .position(|p| p.name == primary_key)
        .ok_or_else(|| StorageError::property_not_found(primary_key.to_string()))?;

    ctx.data_store()
        .register_vertex_type(name.to_string(), None, |label_id| {
            let schema = VertexSchema {
                label_id,
                label_name: name.to_string(),
                properties,
                primary_key_index,
                schema_version: 1,
            };
            schema
                .validate_on_creation()
                .map_err(StorageError::invalid_operation)?;
            Ok(ShardedVertexTable::with_config(
                label_id,
                name.to_string(),
                schema,
                ctx.vertex_table_shards(),
            ))
        })
}

pub fn create_vertex_type_with_id(
    ctx: &GraphStorageContext,
    storage_name: &str,
    user_name: &str,
    label_id: LabelId,
    properties: Vec<StoragePropertyDef>,
    primary_key: &str,
) -> StorageResult<LabelId> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    let primary_key_index = properties
        .iter()
        .position(|p| p.name == primary_key)
        .ok_or_else(|| StorageError::property_not_found(primary_key.to_string()))?;

    ctx.data_store()
        .register_vertex_type(storage_name.to_string(), Some(label_id), |label_id| {
            let schema = VertexSchema {
                label_id,
                label_name: user_name.to_string(),
                properties,
                primary_key_index,
                schema_version: 1,
            };
            schema
                .validate_on_creation()
                .map_err(StorageError::invalid_operation)?;
            Ok(ShardedVertexTable::with_config(
                label_id,
                user_name.to_string(),
                schema,
                ctx.vertex_table_shards(),
            ))
        })
}

pub fn create_edge_type(
    ctx: &GraphStorageContext,
    name: &str,
    src_label: LabelId,
    dst_label: LabelId,
    properties: Vec<StoragePropertyDef>,
    oe_strategy: EdgeStrategy,
    ie_strategy: EdgeStrategy,
) -> StorageResult<LabelId> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    let (source_exists, destination_exists) =
        ctx.data_store().with_vertex_tables(|vertex_tables| {
            (
                vertex_tables.contains_key(&src_label),
                vertex_tables.contains_key(&dst_label),
            )
        });
    if !source_exists {
        return Err(StorageError::label_not_found(format!(
            "source label {}",
            src_label
        )));
    }
    if !destination_exists {
        return Err(StorageError::label_not_found(format!(
            "destination label {}",
            dst_label
        )));
    }

    ctx.data_store()
        .register_edge_type(name.to_string(), None, src_label, dst_label, |label_id| {
            let schema = EdgeSchema {
                label_id,
                label_name: name.to_string(),
                src_label,
                dst_label,
                properties,
                oe_strategy,
                ie_strategy,
                schema_version: 1,
            };
            schema.validate_on_creation()?;
            let mut table = EdgeStore::new(schema)?;
            if let Some(stats) = ctx.stats_manager() {
                table.set_stats_manager(stats.clone());
            }
            Ok(table)
        })
}

pub fn create_edge_type_with_id(
    ctx: &GraphStorageContext,
    params: CreateEdgeTypeParams,
    label_id: LabelId,
) -> StorageResult<LabelId> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    let CreateEdgeTypeParams {
        name,
        user_name,
        src_label,
        dst_label,
        properties,
        oe_strategy,
        ie_strategy,
    } = params;
    ctx.data_store()
        .register_edge_type(
            name.to_string(),
            Some(label_id),
            src_label,
            dst_label,
            |label_id| {
                let schema = EdgeSchema {
                    label_id,
                    label_name: user_name.to_string(),
                    src_label,
                    dst_label,
                    properties,
                    oe_strategy,
                    ie_strategy,
                    schema_version: 1,
                };
                schema.validate_on_creation()?;
                let mut table = EdgeStore::new(schema)?;
                if let Some(stats) = ctx.stats_manager() {
                    table.set_stats_manager(stats.clone());
                }
                Ok(table)
            },
        )
}

pub fn drop_vertex_type(ctx: &GraphStorageContext, name: &str) -> StorageResult<()> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    let label_id = ctx.data_store().drop_vertex_type(name)?;

    ctx.invalidate_vertex_cache(label_id);

    Ok(())
}

pub fn drop_edge_type(ctx: &GraphStorageContext, name: &str) -> StorageResult<()> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    ctx.data_store().drop_edge_type(name)?;
    Ok(())
}

pub fn add_vertex_property(
    ctx: &GraphStorageContext,
    label: LabelId,
    prop: crate::storage::types::StoragePropertyDef,
) -> StorageResult<()> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    ctx.data_store()
        .with_vertex_table_mut(label, |table| table.add_property(prop))
}

pub fn delete_vertex_property(
    ctx: &GraphStorageContext,
    label: LabelId,
    prop_name: &str,
) -> StorageResult<()> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    ctx.data_store()
        .with_vertex_table_mut(label, |table| table.remove_property(prop_name))
}

pub fn rename_vertex_property(
    ctx: &GraphStorageContext,
    label: LabelId,
    old_name: &str,
    new_name: &str,
) -> StorageResult<()> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    ctx.data_store()
        .with_vertex_table_mut(label, |table| table.rename_property(old_name, new_name))
}

pub fn add_edge_property(
    ctx: &GraphStorageContext,
    edge_label: LabelId,
    prop: StoragePropertyDef,
) -> StorageResult<()> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    ctx.data_store()
        .for_each_edge_partition_mut(edge_label, |_key, table| {
            table.add_property(prop.name.clone(), prop.data_type.clone(), prop.nullable)
        })?;
    Ok(())
}

pub fn delete_edge_property(
    ctx: &GraphStorageContext,
    edge_label: LabelId,
    prop_name: &str,
) -> StorageResult<()> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    ctx.data_store()
        .for_each_edge_partition_mut(edge_label, |_key, table| table.remove_property(prop_name))?;
    Ok(())
}

pub fn rename_edge_property(
    ctx: &GraphStorageContext,
    edge_label: LabelId,
    old_name: &str,
    new_name: &str,
) -> StorageResult<()> {
    if !ctx.is_open_flag().load(Ordering::Acquire) {
        return Err(StorageError::storage_not_open());
    }

    ctx.data_store()
        .for_each_edge_partition_mut(edge_label, |_key, table| {
            table.rename_property(old_name, new_name)
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::core::DataType;
    use crate::storage::edge::EdgeStrategy;
    use crate::storage::types::StoragePropertyDef;

    use super::super::GraphStorageContext;

    #[test]
    fn test_create_vertex_type() {
        let ctx = GraphStorageContext::new();
        let props = vec![StoragePropertyDef::new(
            "name".to_string(),
            DataType::String,
        )];
        let label_id = ctx
            .create_vertex_type("Person", props, "name")
            .expect("create_vertex_type should succeed");
        assert_eq!(label_id, 0);
    }

    #[test]
    fn test_create_duplicate_vertex_type() {
        let ctx = GraphStorageContext::new();
        let props = vec![StoragePropertyDef::new(
            "name".to_string(),
            DataType::String,
        )];
        ctx.create_vertex_type("Person", props, "name")
            .expect("create_vertex_type should succeed");
        let props2 = vec![StoragePropertyDef::new(
            "name".to_string(),
            DataType::String,
        )];
        let result = ctx.create_vertex_type("Person", props2, "name");
        assert!(result.is_err());
    }

    #[test]
    fn test_create_vertex_type_missing_primary_key() {
        let ctx = GraphStorageContext::new();
        let result = ctx.create_vertex_type(
            "Person",
            vec![StoragePropertyDef::new(
                "name".to_string(),
                DataType::String,
            )],
            "nonexistent",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_create_edge_type() {
        let ctx = GraphStorageContext::new();
        let props = vec![StoragePropertyDef::new(
            "name".to_string(),
            DataType::String,
        )];
        ctx.create_vertex_type("Person", props, "name")
            .expect("create_vertex_type should succeed");

        let edge_label_id = ctx
            .create_edge_type(
                "KNOWS",
                0,
                0,
                vec![StoragePropertyDef::new("since".to_string(), DataType::Int)],
                EdgeStrategy::Multiple,
                EdgeStrategy::Multiple,
            )
            .expect("create_edge_type should succeed");
        assert_eq!(edge_label_id, 0);
    }

    #[test]
    fn test_create_edge_type_missing_src_label() {
        let ctx = GraphStorageContext::new();
        let result = ctx.create_edge_type(
            "KNOWS",
            0,
            0,
            vec![],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        );
        assert!(result.is_err());
    }

    fn name_prop() -> Vec<StoragePropertyDef> {
        vec![StoragePropertyDef::new(
            "name".to_string(),
            DataType::String,
        )]
    }

    #[test]
    fn test_drop_vertex_type() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");
        assert!(ctx.data_store().vertex_label_id("Person").is_some());
        ctx.drop_vertex_type("Person").expect("drop should succeed");
        assert!(ctx.data_store().vertex_label_id("Person").is_none());
    }

    #[test]
    fn test_drop_nonexistent_vertex_type() {
        let ctx = GraphStorageContext::new();
        let result = ctx.drop_vertex_type("NonExistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_drop_edge_type() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");
        ctx.create_edge_type(
            "KNOWS",
            0,
            0,
            vec![],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        )
        .expect("create_edge_type should succeed");
        ctx.drop_edge_type("KNOWS").expect("drop should succeed");
    }

    #[test]
    fn test_add_vertex_property() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");
        ctx.add_vertex_property(
            0,
            StoragePropertyDef::new("email".to_string(), DataType::String),
        )
        .expect("add_vertex_property should succeed");
    }

    #[test]
    fn test_add_edge_property() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");
        ctx.create_edge_type(
            "KNOWS",
            0,
            0,
            vec![],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        )
        .expect("create_edge_type should succeed");
        ctx.add_edge_property(
            0,
            StoragePropertyDef::new("weight".to_string(), DataType::Double),
        )
        .expect("add_edge_property should succeed");
    }

    #[test]
    fn test_delete_vertex_property() {
        let ctx = GraphStorageContext::new();
        let props = vec![
            StoragePropertyDef::new("name".to_string(), DataType::String),
            StoragePropertyDef::new("age".to_string(), DataType::BigInt),
        ];
        ctx.create_vertex_type("Person", props, "name")
            .expect("create_vertex_type should succeed");
        ctx.delete_vertex_property(0, "age")
            .expect("delete_vertex_property should succeed");
    }

    #[test]
    fn test_rename_vertex_property() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");
        ctx.rename_vertex_property(0, "name", "full_name")
            .expect("rename_vertex_property should succeed");
    }

    #[test]
    fn test_delete_edge_property() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");
        ctx.create_edge_type(
            "KNOWS",
            0,
            0,
            vec![StoragePropertyDef::new("since".to_string(), DataType::Int)],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        )
        .expect("create_edge_type should succeed");
        ctx.delete_edge_property(0, "since")
            .expect("delete_edge_property should succeed");
    }

    #[test]
    fn test_create_vertex_type_with_id() {
        let ctx = GraphStorageContext::new();
        let label_id = ctx
            .create_vertex_type_with_id("space_1:tag:Person", "Person", 42, name_prop(), "name")
            .expect("create_vertex_type_with_id should succeed");
        assert_eq!(label_id, 42);
    }

    #[test]
    fn test_add_vertex_property_label_not_found() {
        let ctx = GraphStorageContext::new();
        let result = ctx.add_vertex_property(
            999,
            StoragePropertyDef::new("email".to_string(), DataType::String),
        );
        assert!(result.is_err());
    }

    // ====== New Validation Tests ======

    #[test]
    fn test_primary_key_type_validation_valid_types() {
        let ctx = GraphStorageContext::new();

        // Test valid key types
        let valid_types = vec![
            ("Bool", DataType::Bool),
            ("SmallInt", DataType::SmallInt),
            ("Int", DataType::Int),
            ("BigInt", DataType::BigInt),
            ("Float", DataType::Float),
            ("Double", DataType::Double),
            ("String", DataType::String),
            ("Uuid", DataType::Uuid),
            ("Date", DataType::Date),
            ("DateTime", DataType::DateTime),
        ];

        for (idx, (type_name, data_type)) in valid_types.into_iter().enumerate() {
            let label_id = ctx
                .create_vertex_type(
                    &format!("Type{}", idx),
                    vec![StoragePropertyDef::new("id".to_string(), data_type)],
                    "id",
                )
                .unwrap_or_else(|_| panic!("Should accept {} as primary key type", type_name));
            assert_eq!(label_id, idx as u32);
        }
    }

    #[test]
    fn test_primary_key_type_validation_invalid_types() {
        let ctx = GraphStorageContext::new();

        // Test invalid key types - composite/complex types
        let invalid_types = vec![
            ("List", DataType::List),
            ("Map", DataType::Map),
            ("Set", DataType::Set),
            ("Vertex", DataType::Vertex),
            ("Edge", DataType::Edge),
            ("Path", DataType::Path),
            ("Vector", DataType::Vector),
        ];

        for (type_name, data_type) in invalid_types {
            let result = ctx.create_vertex_type(
                &format!("BadType{}", type_name),
                vec![StoragePropertyDef::new("id".to_string(), data_type)],
                "id",
            );
            assert!(
                result.is_err(),
                "Should reject {} as primary key type",
                type_name
            );
        }
    }

    #[test]
    fn test_property_type_empty_invalid() {
        let ctx = GraphStorageContext::new();
        let result = ctx.create_vertex_type(
            "BadSchema",
            vec![
                StoragePropertyDef::new("id".to_string(), DataType::String),
                StoragePropertyDef {
                    name: "bad_prop".to_string(),
                    data_type: DataType::Empty,
                    nullable: false,
                    default_value: None,
                },
            ],
            "id",
        );
        assert!(result.is_err(), "Should reject Empty type property");
    }

    #[test]
    fn test_property_type_null_invalid() {
        let ctx = GraphStorageContext::new();
        let result = ctx.create_vertex_type(
            "BadSchema",
            vec![
                StoragePropertyDef::new("id".to_string(), DataType::String),
                StoragePropertyDef {
                    name: "bad_prop".to_string(),
                    data_type: DataType::Null,
                    nullable: false,
                    default_value: None,
                },
            ],
            "id",
        );
        assert!(result.is_err(), "Should reject Null type property");
    }

    #[test]
    fn test_invalid_property_names() {
        let ctx = GraphStorageContext::new();

        // Test property name starting with number
        let result = ctx.create_vertex_type(
            "BadNames",
            vec![
                StoragePropertyDef::new("id".to_string(), DataType::String),
                StoragePropertyDef::new("123prop".to_string(), DataType::String),
            ],
            "id",
        );
        assert!(
            result.is_err(),
            "Property name should not start with number"
        );

        // Test property name with invalid characters
        let result = ctx.create_vertex_type(
            "BadNames2",
            vec![
                StoragePropertyDef::new("id".to_string(), DataType::String),
                StoragePropertyDef::new("prop-name".to_string(), DataType::String),
            ],
            "id",
        );
        assert!(result.is_err(), "Property name should not contain hyphens");
    }

    #[test]
    fn test_valid_property_names() {
        let ctx = GraphStorageContext::new();

        // Valid names: starting with letter or underscore, containing alphanumeric and underscore
        let _label_id = ctx
            .create_vertex_type(
                "ValidNames",
                vec![
                    StoragePropertyDef::new("id".to_string(), DataType::String),
                    StoragePropertyDef::new("_internal".to_string(), DataType::String),
                    StoragePropertyDef::new("field_2".to_string(), DataType::String),
                    StoragePropertyDef::new("CamelCase".to_string(), DataType::String),
                ],
                "id",
            )
            .expect("All property names are valid");
    }

    #[test]
    fn test_edge_schema_both_strategies_none() {
        let ctx = GraphStorageContext::new();
        // Create vertex types first
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");

        // Try to create edge with both strategies None
        let result = ctx.create_edge_type(
            "BadEdge",
            0,
            0,
            vec![],
            EdgeStrategy::None,
            EdgeStrategy::None,
        );
        assert!(result.is_err(), "Edge cannot have both strategies as None");
    }

    #[test]
    fn test_edge_property_type_validation() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");

        // Create edge with valid properties
        let label_id = ctx
            .create_edge_type(
                "ValidEdge",
                0,
                0,
                vec![
                    StoragePropertyDef::new("weight".to_string(), DataType::Double),
                    StoragePropertyDef::new("since".to_string(), DataType::DateTime),
                ],
                EdgeStrategy::Multiple,
                EdgeStrategy::Multiple,
            )
            .expect("Edge with valid properties");
        assert_eq!(label_id, 0);

        // Try edge with Empty type property
        let result = ctx.create_edge_type(
            "BadEdge",
            0,
            0,
            vec![StoragePropertyDef {
                name: "bad".to_string(),
                data_type: DataType::Empty,
                nullable: false,
                default_value: None,
            }],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        );
        assert!(result.is_err(), "Edge should reject Empty type property");
    }

    #[test]
    fn test_add_vertex_property_increments_version() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");

        // Check initial version is 1
        let initial_version = ctx
            .data_store()
            .test_read_vertex_tables()
            .get(&0)
            .unwrap()
            .schema()
            .schema_version;
        assert_eq!(initial_version, 1, "Initial version should be 1");

        // Add property and verify version incremented
        ctx.add_vertex_property(
            0,
            StoragePropertyDef::new("email".to_string(), DataType::String),
        )
        .expect("add_vertex_property should succeed");

        let updated_version = ctx
            .data_store()
            .test_read_vertex_tables()
            .get(&0)
            .unwrap()
            .schema()
            .schema_version;
        assert_eq!(
            updated_version, 2,
            "Version should increment to 2 after add_property"
        );
    }

    #[test]
    fn test_delete_vertex_property_increments_version() {
        let ctx = GraphStorageContext::new();
        let props = vec![
            StoragePropertyDef::new("name".to_string(), DataType::String),
            StoragePropertyDef::new("age".to_string(), DataType::BigInt),
        ];
        ctx.create_vertex_type("Person", props, "name")
            .expect("create_vertex_type should succeed");

        let v1 = ctx
            .data_store()
            .test_read_vertex_tables()
            .get(&0)
            .unwrap()
            .schema()
            .schema_version;

        ctx.delete_vertex_property(0, "age")
            .expect("delete_vertex_property should succeed");

        let v2 = ctx
            .data_store()
            .test_read_vertex_tables()
            .get(&0)
            .unwrap()
            .schema()
            .schema_version;

        assert_eq!(v2, v1 + 1, "Version should increment after delete_property");
    }

    #[test]
    fn test_rename_vertex_property_increments_version() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");

        let v1 = ctx
            .data_store()
            .test_read_vertex_tables()
            .get(&0)
            .unwrap()
            .schema()
            .schema_version;

        ctx.rename_vertex_property(0, "name", "full_name")
            .expect("rename_vertex_property should succeed");

        let v2 = ctx
            .data_store()
            .test_read_vertex_tables()
            .get(&0)
            .unwrap()
            .schema()
            .schema_version;

        assert_eq!(v2, v1 + 1, "Version should increment after rename_property");
    }

    #[test]
    fn test_vertex_schema_version_increments_sequentially() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");

        for expected_version in 2..=5 {
            ctx.add_vertex_property(
                0,
                StoragePropertyDef::new(format!("prop{}", expected_version), DataType::String),
            )
            .expect("add_vertex_property should succeed");

            let actual_version = ctx
                .data_store()
                .test_read_vertex_tables()
                .get(&0)
                .unwrap()
                .schema()
                .schema_version;

            assert_eq!(
                actual_version, expected_version,
                "Version should increment sequentially"
            );
        }
    }

    #[test]
    fn test_add_edge_property_increments_version() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");
        ctx.create_edge_type(
            "KNOWS",
            0,
            0,
            vec![],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        )
        .expect("create_edge_type should succeed");

        // Check initial version is 1
        let initial_version = ctx
            .data_store()
            .test_read_edge_tables()
            .values()
            .next()
            .unwrap()
            .read()
            .schema()
            .schema_version;
        assert_eq!(initial_version, 1, "Initial version should be 1");

        ctx.add_edge_property(
            0,
            StoragePropertyDef::new("weight".to_string(), DataType::Double),
        )
        .expect("add_edge_property should succeed");

        let updated_version = ctx
            .data_store()
            .test_read_edge_tables()
            .values()
            .next()
            .unwrap()
            .read()
            .schema()
            .schema_version;

        assert_eq!(
            updated_version, 2,
            "Version should increment to 2 after add_edge_property"
        );
    }

    #[test]
    fn test_delete_edge_property_increments_version() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");
        ctx.create_edge_type(
            "KNOWS",
            0,
            0,
            vec![StoragePropertyDef::new(
                "weight".to_string(),
                DataType::Double,
            )],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        )
        .expect("create_edge_type should succeed");

        let v1 = ctx
            .data_store()
            .test_read_edge_tables()
            .values()
            .next()
            .unwrap()
            .read()
            .schema()
            .schema_version;

        ctx.delete_edge_property(0, "weight")
            .expect("delete_edge_property should succeed");

        let v2 = ctx
            .data_store()
            .test_read_edge_tables()
            .values()
            .next()
            .unwrap()
            .read()
            .schema()
            .schema_version;

        assert_eq!(
            v2,
            v1 + 1,
            "Version should increment after delete_edge_property"
        );
    }

    #[test]
    fn test_rename_edge_property_increments_version() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");
        ctx.create_edge_type(
            "KNOWS",
            0,
            0,
            vec![StoragePropertyDef::new(
                "weight".to_string(),
                DataType::Double,
            )],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        )
        .expect("create_edge_type should succeed");

        let v1 = ctx
            .data_store()
            .test_read_edge_tables()
            .values()
            .next()
            .unwrap()
            .read()
            .schema()
            .schema_version;

        ctx.rename_edge_property(0, "weight", "strength")
            .expect("rename_edge_property should succeed");

        let v2 = ctx
            .data_store()
            .test_read_edge_tables()
            .values()
            .next()
            .unwrap()
            .read()
            .schema()
            .schema_version;

        assert_eq!(
            v2,
            v1 + 1,
            "Version should increment after rename_edge_property"
        );
    }

    #[test]
    fn test_edge_schema_version_increments_sequentially() {
        let ctx = GraphStorageContext::new();
        ctx.create_vertex_type("Person", name_prop(), "name")
            .expect("create_vertex_type should succeed");
        ctx.create_edge_type(
            "KNOWS",
            0,
            0,
            vec![],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        )
        .expect("create_edge_type should succeed");

        for expected_version in 2..=4 {
            ctx.add_edge_property(
                0,
                StoragePropertyDef::new(format!("prop{}", expected_version), DataType::String),
            )
            .expect("add_edge_property should succeed");

            let actual_version = ctx
                .data_store()
                .test_read_edge_tables()
                .values()
                .next()
                .unwrap()
                .read()
                .schema()
                .schema_version;

            assert_eq!(
                actual_version, expected_version,
                "Edge version should increment sequentially"
            );
        }
    }
}
