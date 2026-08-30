use crate::edge::EdgeStrategy;
use crate::engine::graph_storage::GraphStorageContext;
use crate::engine::params::CreateEdgeTypeParams;
use crate::types::StoragePropertyDef;
use graphdb_core::error::storage::StorageErrorKind;
use graphdb_core::types::{
    DataType, EdgeTypeInfo, LabelId, PropertyDef, SpaceInfo, TagInfo, Timestamp,
};
use graphdb_core::{StorageError, StorageResult};
use graphdb_transaction::wal::{
    AddEdgePropRedo, AddVertexPropRedo, AlterSpaceCommentRedo, ClearSpaceRedo, CreateEdgeTypeRedo,
    CreateSpaceRedo, CreateVertexTypeRedo, DeleteEdgePropRedo, DeleteEdgeTypeRedo,
    DeleteVertexPropRedo, DeleteVertexTypeRedo, RenameEdgePropRedo, RenameVertexPropRedo,
};

pub(crate) fn replay_create_space(
    ctx: &GraphStorageContext,
    redo: &CreateSpaceRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let mut space = redo.space.clone();
    let _ = ctx.schema_manager().create_space(&mut space)?;
    Ok(())
}

pub(crate) fn replay_drop_space(
    ctx: &GraphStorageContext,
    redo: &graphdb_transaction::wal::DropSpaceRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let Some(space_info) = ctx.schema_manager().get_space(&redo.space_name)? else {
        return Ok(());
    };

    let space_id = space_info.space_id;
    let tags = ctx.schema_manager().list_tags(&redo.space_name)?;
    let edge_types = ctx.schema_manager().list_edge_types(&redo.space_name)?;

    for edge_type in edge_types {
        let storage_name = format!("space_{space_id}:edge:{}", edge_type.edge_type_name);
        let _ = ctx.drop_edge_type(&storage_name);
    }
    for tag in tags {
        let storage_name = format!("space_{space_id}:tag:{}", tag.tag_name);
        let _ = ctx.drop_vertex_type(&storage_name);
    }

    let _ = ctx.schema_manager().drop_space(&redo.space_name)?;
    Ok(())
}

pub(crate) fn replay_clear_space(
    ctx: &GraphStorageContext,
    redo: &ClearSpaceRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let Some(space_info) = ctx.schema_manager().get_space(&redo.space_name)? else {
        return Ok(());
    };

    let space_id = space_info.space_id;
    let tags = ctx.schema_manager().list_tags(&redo.space_name)?;
    let edge_types = ctx.schema_manager().list_edge_types(&redo.space_name)?;

    for edge_type in edge_types {
        let storage_name = format!("space_{space_id}:edge:{}", edge_type.edge_type_name);
        let _ = ctx.drop_edge_type(&storage_name);
    }
    for tag in tags {
        let storage_name = format!("space_{space_id}:tag:{}", tag.tag_name);
        let _ = ctx.drop_vertex_type(&storage_name);
    }

    let _ = ctx.schema_manager().clear_space(&redo.space_name)?;
    Ok(())
}

pub(crate) fn replay_alter_space_comment(
    ctx: &GraphStorageContext,
    redo: &AlterSpaceCommentRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let _ = ctx
        .schema_manager()
        .alter_space_comment(redo.space_id, redo.comment.clone())?;
    Ok(())
}

pub(crate) fn replay_create_vertex_type(
    ctx: &GraphStorageContext,
    redo: &CreateVertexTypeRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let mut properties = Vec::with_capacity(redo.schema.len());
    for (name, type_name, _serial) in &redo.schema {
        properties.push(StoragePropertyDef::new(
            name.clone(),
            parse_data_type(type_name)?,
        ));
    }

    if properties.is_empty() {
        log::warn!(
            "replay_create_vertex_type skipped because schema is empty: {}",
            redo.label_name
        );
        return Ok(());
    }

    let primary_key = properties
        .first()
        .map(|prop| prop.name.clone())
        .unwrap_or_else(|| redo.label_name.clone());

    ctx.ensure_recovery_space(&redo.space_name)?;

    let space_id = ctx
        .schema_manager()
        .get_space_id(&redo.space_name)
        .unwrap_or(0);
    let label_id = if let Some(label_id) = redo.label_id {
        let storage_name = format!("space_{space_id}:tag:{}", redo.label_name);
        match ctx.create_vertex_type_with_id(
            &storage_name,
            &redo.label_name,
            label_id,
            properties.clone(),
            &primary_key,
        ) {
            Ok(id) => id,
            Err(e) if e.kind() == StorageErrorKind::LabelAlreadyExists => label_id,
            Err(e) => return Err(e),
        }
    } else {
        ctx.create_vertex_type(&redo.label_name, properties.clone(), &primary_key)?
    };
    let tag = TagInfo::new(redo.label_name.clone()).with_properties(
        redo.schema
            .iter()
            .map(|(name, type_name, serial)| {
                parse_data_type(type_name).map(|data_type| {
                    PropertyDef::new(name.clone(), data_type)
                        .with_nullable(false)
                        .with_serial(*serial)
                })
            })
            .collect::<StorageResult<Vec<_>>>()?,
    );
    match ctx
        .schema_manager()
        .create_tag_with_id(&redo.space_name, &tag, label_id)
    {
        Ok(_) => {}
        Err(e) if e.kind() == StorageErrorKind::LabelAlreadyExists => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

pub(crate) fn replay_create_edge_type(
    ctx: &GraphStorageContext,
    redo: &CreateEdgeTypeRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let get_label_id = |tag_name: &str| -> StorageResult<LabelId> {
        if tag_name.is_empty() {
            return Ok(0);
        }
        ctx.schema_manager()
            .get_tag(&redo.space_name, tag_name)?
            .map(|t| t.tag_id)
            .ok_or_else(|| {
                StorageError::db_error(format!(
                    "Source vertex label not found during recovery: {}",
                    tag_name
                ))
            })
    };
    let src_label = get_label_id(&redo.src_label)?;
    let dst_label = get_label_id(&redo.dst_label)?;

    let mut properties = Vec::with_capacity(redo.schema.len());
    for (name, type_name, _serial) in &redo.schema {
        properties.push(StoragePropertyDef::new(
            name.clone(),
            parse_data_type(type_name)?,
        ));
    }

    ctx.ensure_recovery_space(&redo.space_name)?;

    let space_id = ctx
        .schema_manager()
        .get_space_id(&redo.space_name)
        .unwrap_or(0);
    let label_id = if let Some(label_id) = redo.label_id {
        let _space_id = ctx
            .schema_manager()
            .get_space_id(&redo.space_name)
            .unwrap_or(0);
        let storage_name = format!("space_{space_id}:edge:{}", redo.edge_label);
        match ctx.create_edge_type_with_id(
            CreateEdgeTypeParams {
                name: &storage_name,
                user_name: &redo.edge_label,
                src_label,
                dst_label,
                properties,
                oe_strategy: EdgeStrategy::Multiple,
                ie_strategy: EdgeStrategy::Multiple,
            },
            label_id,
        ) {
            Ok(id) => id,
            Err(e) if e.kind() == StorageErrorKind::LabelAlreadyExists => label_id,
            Err(e) => return Err(e),
        }
    } else {
        ctx.create_edge_type(
            &redo.edge_label,
            src_label,
            dst_label,
            properties,
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        )?
    };
    let edge_type = EdgeTypeInfo::new(redo.edge_label.clone())
        .with_src_tag(redo.src_label.clone())
        .with_dst_tag(redo.dst_label.clone())
        .with_properties(
            redo.schema
                .iter()
                .map(|(name, type_name, serial)| {
                    parse_data_type(type_name).map(|data_type| {
                        PropertyDef::new(name.clone(), data_type)
                            .with_nullable(false)
                            .with_serial(*serial)
                    })
                })
                .collect::<StorageResult<Vec<_>>>()?,
        );
    match ctx
        .schema_manager()
        .create_edge_type_with_id(&redo.space_name, &edge_type, label_id)
    {
        Ok(_) => {}
        Err(e) if e.kind() == StorageErrorKind::LabelAlreadyExists => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

pub(crate) fn replay_delete_vertex_type(
    ctx: &GraphStorageContext,
    redo: &DeleteVertexTypeRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let _space_name = redo.space_name.as_deref().unwrap_or("");
    if let Some(space_name) = &redo.space_name {
        if let Ok(Some(space_info)) = ctx.schema_manager().get_space(space_name) {
            let storage_name = format!("space_{}:tag:{}", space_info.space_id, redo.label_name);
            ctx.drop_vertex_type(&storage_name)?;
        }
    }
    if let Some(space_name) = &redo.space_name {
        let _ = ctx.schema_manager().drop_tag(space_name, &redo.label_name);
    }
    Ok(())
}

pub(crate) fn replay_delete_edge_type(
    ctx: &GraphStorageContext,
    redo: &DeleteEdgeTypeRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let _space_name = redo.space_name.as_deref().unwrap_or("");
    if let Some(space_name) = &redo.space_name {
        if let Ok(Some(space_info)) = ctx.schema_manager().get_space(space_name) {
            let storage_name = format!("space_{}:edge:{}", space_info.space_id, redo.edge_label);
            ctx.drop_edge_type(&storage_name)?;
        }
    }
    if let Some(space_name) = &redo.space_name {
        let _ = ctx
            .schema_manager()
            .drop_edge_type(space_name, &redo.edge_label);
    }
    Ok(())
}

pub(crate) fn replay_add_vertex_prop(
    ctx: &GraphStorageContext,
    redo: &AddVertexPropRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let mut props = Vec::with_capacity(redo.properties.len());
    for (name, type_name, _serial) in &redo.properties {
        props.push(StoragePropertyDef::new(
            name.clone(),
            parse_data_type(type_name)?,
        ));
    }

    let mut added_props = Vec::new();
    for prop in props {
        match ctx.add_vertex_property(redo.label, prop.clone()) {
            Ok(()) => {
                added_props.push((prop.name, prop.data_type));
            }
            Err(e) => {
                if e.to_string().contains("already exists") {
                    ctx.data_store().with_vertex_tables_mut(|vertex_tables| {
                        if let Some(table) = vertex_tables.get(&redo.label) {
                            let change_details = crate::schema::ChangeDetails::PropertyAdded {
                                name: prop.name.clone(),
                                data_type: prop.data_type.clone(),
                                nullable: prop.nullable,
                                default_value: None,
                            };
                            table.rebuild_schema_change_from_redo(change_details)?;
                            added_props.push((prop.name, prop.data_type));
                        }
                        Ok(())
                    })?;
                } else {
                    return Err(e);
                }
            }
        }
    }

    if let Some((space_name, mut tag)) = ctx.schema_manager().find_tag_by_id(redo.label) {
        for (name, type_name, serial) in &redo.properties {
            let prop = PropertyDef::new(name.clone(), parse_data_type(type_name)?)
                .with_nullable(false)
                .with_serial(*serial);
            if !tag
                .properties
                .iter()
                .any(|existing| existing.name == prop.name)
            {
                tag.properties.push(prop);
            }
        }
        ctx.schema_manager().update_tag(&space_name, &tag)?;
    }
    Ok(())
}

pub(crate) fn replay_add_edge_prop(
    ctx: &GraphStorageContext,
    redo: &AddEdgePropRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let mut props = Vec::with_capacity(redo.properties.len());
    for (name, type_name, _serial) in &redo.properties {
        props.push(StoragePropertyDef::new(
            name.clone(),
            parse_data_type(type_name)?,
        ));
    }

    for prop in props {
        match ctx.add_edge_property(redo.edge_label, prop.clone()) {
            Ok(()) => {}
            Err(e) => {
                if e.to_string().contains("already exists") {
                    let key = ctx.data_store().with_edge_label_index(|edge_label_index| {
                        edge_label_index
                            .get(&redo.edge_label)
                            .and_then(|keys| keys.first().copied())
                    });
                    let Some(key) = key else {
                        return Err(e);
                    };
                    let arc = ctx
                        .data_store()
                        .with_edge_tables(|tables| tables.get(&key).cloned());
                    if let Some(arc) = arc {
                        let mut table = arc.write();
                        let change_details = crate::schema::ChangeDetails::PropertyAdded {
                            name: prop.name.clone(),
                            data_type: prop.data_type.clone(),
                            nullable: prop.nullable,
                            default_value: None,
                        };
                        table.rebuild_schema_change_from_redo(change_details)?;
                    }
                } else {
                    return Err(e);
                }
            }
        }
    }

    if let Some((space_name, mut edge_type)) =
        ctx.schema_manager().find_edge_type_by_id(redo.edge_label)
    {
        for (name, type_name, serial) in &redo.properties {
            let prop = PropertyDef::new(name.clone(), parse_data_type(type_name)?)
                .with_nullable(false)
                .with_serial(*serial);
            if !edge_type
                .properties
                .iter()
                .any(|existing| existing.name == prop.name)
            {
                edge_type.properties.push(prop);
            }
        }
        ctx.schema_manager()
            .update_edge_type(&space_name, &edge_type)?;
    }
    Ok(())
}

pub(crate) fn replay_delete_vertex_prop(
    ctx: &GraphStorageContext,
    redo: &DeleteVertexPropRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let (space_name, mut tag) = ctx
        .schema_manager()
        .find_tag_by_id(redo.label)
        .ok_or_else(|| StorageError::label_not_found(format!("vertex label {}", redo.label)))?;

    tag.properties
        .retain(|prop| !redo.prop_names.iter().any(|name| name == &prop.name));
    ctx.schema_manager().update_tag(&space_name, &tag)?;

    for prop_name in &redo.prop_names {
        ctx.delete_vertex_property(redo.label, prop_name)?;
    }
    Ok(())
}

pub(crate) fn replay_delete_edge_prop(
    ctx: &GraphStorageContext,
    redo: &DeleteEdgePropRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let (space_name, mut edge_type) = ctx
        .schema_manager()
        .find_edge_type_by_id(redo.edge_label)
        .ok_or_else(|| StorageError::label_not_found(format!("edge label {}", redo.edge_label)))?;

    edge_type
        .properties
        .retain(|prop| !redo.prop_names.iter().any(|name| name == &prop.name));
    ctx.schema_manager()
        .update_edge_type(&space_name, &edge_type)?;

    for prop_name in &redo.prop_names {
        ctx.delete_edge_property(redo.edge_label, prop_name)?;
    }
    Ok(())
}

pub(crate) fn replay_rename_vertex_prop(
    ctx: &GraphStorageContext,
    redo: &RenameVertexPropRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let (space_name, mut tag) = ctx
        .schema_manager()
        .find_tag_by_id(redo.label)
        .ok_or_else(|| StorageError::label_not_found(format!("vertex label {}", redo.label)))?;

    let prop = tag
        .properties
        .iter_mut()
        .find(|prop| prop.name == redo.old_name)
        .ok_or_else(|| StorageError::column_not_found(redo.old_name.clone()))?;
    prop.name = redo.new_name.clone();

    ctx.schema_manager().update_tag(&space_name, &tag)?;
    ctx.rename_vertex_property(redo.label, &redo.old_name, &redo.new_name)?;
    Ok(())
}

pub(crate) fn replay_rename_edge_prop(
    ctx: &GraphStorageContext,
    redo: &RenameEdgePropRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let (space_name, mut edge_type) = ctx
        .schema_manager()
        .find_edge_type_by_id(redo.edge_label)
        .ok_or_else(|| StorageError::label_not_found(format!("edge label {}", redo.edge_label)))?;

    let prop = edge_type
        .properties
        .iter_mut()
        .find(|prop| prop.name == redo.old_name)
        .ok_or_else(|| StorageError::column_not_found(redo.old_name.clone()))?;
    prop.name = redo.new_name.clone();

    ctx.schema_manager()
        .update_edge_type(&space_name, &edge_type)?;
    ctx.rename_edge_property(redo.edge_label, &redo.old_name, &redo.new_name)?;
    Ok(())
}

impl GraphStorageContext {
    pub(crate) fn ensure_recovery_space(&self, space_name: &str) -> StorageResult<()> {
        if self.schema_manager().get_space(space_name)?.is_some() {
            return Ok(());
        }

        let mut space = SpaceInfo::new(space_name.to_string());
        self.schema_manager().create_space(&mut space)?;
        Ok(())
    }
}

pub(crate) fn parse_data_type(raw: &str) -> StorageResult<DataType> {
    let upper = raw.trim().to_ascii_uppercase();

    let ty = match upper.as_str() {
        "EMPTY" => DataType::Empty,
        "NULL" => DataType::Null,
        "BOOL" => DataType::Bool,
        "SMALLINT" => DataType::SmallInt,
        "INT" => DataType::Int,
        "BIGINT" => DataType::BigInt,
        "FLOAT" => DataType::Float,
        "DOUBLE" => DataType::Double,
        "DECIMAL128" => DataType::Decimal128,
        "STRING" => DataType::String,
        "DATE" => DataType::Date,
        "TIME" => DataType::Time,
        "DATETIME" => DataType::DateTime,
        "VERTEX" => DataType::Vertex,
        "EDGE" => DataType::Edge,
        "PATH" => DataType::Path,
        "LIST" => DataType::List(Box::new(DataType::Empty)),
        "MAP" => DataType::Map(Box::new(DataType::Empty)),
        "SET" => DataType::Set(Box::new(DataType::Empty)),
        "GEOGRAPHY" => DataType::Geography,
        "DATASET" => DataType::DataSet,
        "BLOB" => DataType::Blob,
        "TIMESTAMP" => DataType::DateTime,
        "VECTOR" => DataType::Vector,
        "JSON" => DataType::Json,
        "JSONB" => DataType::JsonB,
        "UUID" => DataType::Uuid,
        "INTERVAL" => DataType::Interval,
        value if value.starts_with("FIXEDSTRING(") && value.ends_with(')') => {
            let inner = &value["FIXEDSTRING(".len()..value.len() - 1];
            let size = inner.trim().parse::<usize>().map_err(|e| {
                StorageError::deserialize_error(format!(
                    "Invalid FIXEDSTRING size in WAL recovery: {}",
                    e
                ))
            })?;
            DataType::FixedString(size)
        }
        value if value.starts_with("VECTOR_DENSE(") && value.ends_with(')') => {
            let inner = &value["VECTOR_DENSE(".len()..value.len() - 1];
            let size = inner.trim().parse::<usize>().map_err(|e| {
                StorageError::deserialize_error(format!(
                    "Invalid VECTOR_DENSE size in WAL recovery: {}",
                    e
                ))
            })?;
            DataType::VectorDense(size)
        }
        value if value.starts_with("VECTOR_SPARSE(") && value.ends_with(')') => {
            let inner = &value["VECTOR_SPARSE(".len()..value.len() - 1];
            let size = inner.trim().parse::<usize>().map_err(|e| {
                StorageError::deserialize_error(format!(
                    "Invalid VECTOR_SPARSE size in WAL recovery: {}",
                    e
                ))
            })?;
            DataType::VectorSparse(size)
        }
        other => {
            return Err(StorageError::deserialize_error(format!(
                "Unsupported data type in WAL recovery: {}",
                other
            )));
        }
    };

    Ok(ty)
}
