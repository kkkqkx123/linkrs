//! Schema handlers for spaces, tags, edge types and version history.

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::proto::*;
use super::service::GraphDBService;

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > GraphDBService<S>
{
    pub(crate) async fn handle_create_space(
        &self,
        request: Request<CreateSpaceRequest>,
    ) -> Result<Response<CreateSpaceResponse>, Status> {
        let req = request.into_inner();
        if req.name.is_empty() {
            return Err(Status::invalid_argument("space name must not be empty"));
        }
        let mut info = linkrs_core::types::SpaceInfo::new(req.name.clone());
        if let Some(options) = req.options {
            if options.partition_num > 0 {
                info.partition_num = options.partition_num;
            }
            if options.replica_num > 0 {
                info.replica_factor = options.replica_num;
            }
        }
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let created = storage_guard
            .create_space(&mut info)
            .map_err(|e| Status::internal(format!("failed to create space: {e}")))?;
        if !created {
            return Err(Status::already_exists(format!(
                "space '{}' already exists",
                req.name
            )));
        }
        let space_id = storage_guard
            .get_space_id(&req.name)
            .map_err(|e| Status::internal(format!("failed to resolve new space: {e}")))?;
        Ok(Response::new(CreateSpaceResponse {
            success: true,
            space_id: space_id as i32,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_get_space(
        &self,
        request: Request<GetSpaceRequest>,
    ) -> Result<Response<GetSpaceResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let space = storage_guard
            .get_space(&req.name)
            .map_err(|e| Status::internal(format!("failed to get space: {e}")))?;
        match space {
            Some(info) => Ok(Response::new(GetSpaceResponse {
                exists: true,
                space: Some(core_space_to_proto(&info)),
                error: String::new(),
            })),
            None => Ok(Response::new(GetSpaceResponse {
                exists: false,
                space: None,
                error: String::new(),
            })),
        }
    }

    pub(crate) async fn handle_drop_space(
        &self,
        request: Request<DropSpaceRequest>,
    ) -> Result<Response<DropSpaceResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let dropped = storage_guard
            .drop_space(&req.name)
            .map_err(|e| Status::internal(format!("failed to drop space: {e}")))?;
        if dropped || req.if_exists {
            Ok(Response::new(DropSpaceResponse {
                success: true,
                error: String::new(),
            }))
        } else {
            Err(Status::not_found(format!(
                "space '{}' does not exist",
                req.name
            )))
        }
    }

    pub(crate) async fn handle_list_spaces(
        &self,
        _request: Request<ListSpacesRequest>,
    ) -> Result<Response<ListSpacesResponse>, Status> {
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let spaces = storage_guard
            .list_spaces()
            .map_err(|e| Status::internal(format!("failed to list spaces: {e}")))?;
        Ok(Response::new(ListSpacesResponse {
            spaces: spaces.iter().map(core_space_to_proto).collect(),
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_create_tag(
        &self,
        request: Request<CreateTagRequest>,
    ) -> Result<Response<CreateTagResponse>, Status> {
        let req = request.into_inner();
        if req.space_name.is_empty() || req.tag_name.is_empty() {
            return Err(Status::invalid_argument(
                "space_name and tag_name must not be empty",
            ));
        }
        let properties = req
            .properties
            .into_iter()
            .map(proto_property_to_core)
            .collect::<Vec<_>>();
        let mut tag_info =
            linkrs_core::types::TagInfo::new(req.tag_name.clone()).with_properties(properties);
        if let Some(options) = req.options {
            let ttl = (options.ttl_seconds > 0).then_some(options.ttl_seconds);
            let col = (!options.ttl_column.is_empty()).then(|| options.ttl_column.clone());
            tag_info = tag_info.with_ttl(ttl, col);
        }
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let tag_id = storage_guard
            .create_tag(&req.space_name, &tag_info)
            .map_err(|e| Status::internal(format!("failed to create tag: {e}")))?;
        Ok(Response::new(CreateTagResponse {
            success: true,
            tag_id: tag_id as i32,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_get_tag(
        &self,
        request: Request<GetTagRequest>,
    ) -> Result<Response<GetTagResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let tag = storage_guard
            .get_tag(&req.space_name, &req.tag_name)
            .map_err(|e| Status::internal(format!("failed to get tag: {e}")))?;
        match tag {
            Some(info) => Ok(Response::new(GetTagResponse {
                exists: true,
                tag: Some(core_tag_to_proto(&info)),
                error: String::new(),
            })),
            None => Ok(Response::new(GetTagResponse {
                exists: false,
                tag: None,
                error: String::new(),
            })),
        }
    }

    pub(crate) async fn handle_list_tags(
        &self,
        request: Request<ListTagsRequest>,
    ) -> Result<Response<ListTagsResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let tags = storage_guard
            .list_tags(&req.space_name)
            .map_err(|e| Status::internal(format!("failed to list tags: {e}")))?;
        Ok(Response::new(ListTagsResponse {
            tags: tags.iter().map(core_tag_to_proto).collect(),
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_drop_tag(
        &self,
        request: Request<DropTagRequest>,
    ) -> Result<Response<DropTagResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let dropped = storage_guard
            .drop_tag(&req.space_name, &req.tag_name)
            .map_err(|e| Status::internal(format!("failed to drop tag: {e}")))?;
        if dropped || req.if_exists {
            Ok(Response::new(DropTagResponse {
                success: true,
                error: String::new(),
            }))
        } else {
            Err(Status::not_found(format!(
                "tag '{}' does not exist",
                req.tag_name
            )))
        }
    }

    pub(crate) async fn handle_create_edge_type(
        &self,
        request: Request<CreateEdgeTypeRequest>,
    ) -> Result<Response<CreateEdgeTypeResponse>, Status> {
        let req = request.into_inner();
        if req.space_name.is_empty() || req.edge_type_name.is_empty() {
            return Err(Status::invalid_argument(
                "space_name and edge_type_name must not be empty",
            ));
        }
        let properties = req
            .properties
            .into_iter()
            .map(proto_property_to_core)
            .collect::<Vec<_>>();
        let edge_info = linkrs_core::types::EdgeTypeInfo::new(req.edge_type_name.clone())
            .with_properties(properties);
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let edge_type_id = storage_guard
            .create_edge_type(&req.space_name, &edge_info)
            .map_err(|e| Status::internal(format!("Failed to create edge type: {e}")))?;
        Ok(Response::new(CreateEdgeTypeResponse {
            success: true,
            edge_type_id: edge_type_id as i32,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_get_edge_type(
        &self,
        request: Request<GetEdgeTypeRequest>,
    ) -> Result<Response<GetEdgeTypeResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let edge = storage_guard
            .get_edge_type(&req.space_name, &req.edge_type_name)
            .map_err(|e| Status::internal(format!("Failed to get edge type: {e}")))?;
        match edge {
            Some(info) => Ok(Response::new(GetEdgeTypeResponse {
                exists: true,
                edge_type: Some(core_edge_info_to_proto(&info)),
                error: String::new(),
            })),
            None => Ok(Response::new(GetEdgeTypeResponse {
                exists: false,
                edge_type: None,
                error: String::new(),
            })),
        }
    }

    pub(crate) async fn handle_list_edge_types(
        &self,
        request: Request<ListEdgeTypesRequest>,
    ) -> Result<Response<ListEdgeTypesResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let edges = storage_guard
            .list_edge_types(&req.space_name)
            .map_err(|e| Status::internal(format!("Failed to list edge types: {e}")))?;
        Ok(Response::new(ListEdgeTypesResponse {
            edge_types: edges.iter().map(core_edge_info_to_proto).collect(),
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_drop_edge_type(
        &self,
        request: Request<DropEdgeTypeRequest>,
    ) -> Result<Response<DropEdgeTypeResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let dropped = storage_guard
            .drop_edge_type(&req.space_name, &req.edge_type_name)
            .map_err(|e| Status::internal(format!("Failed to drop edge type: {e}")))?;
        if dropped || req.if_exists {
            Ok(Response::new(DropEdgeTypeResponse {
                success: true,
                error: String::new(),
            }))
        } else {
            Ok(Response::new(DropEdgeTypeResponse {
                success: false,
                error: format!("Edge type '{}' does not exist", req.edge_type_name),
            }))
        }
    }

    pub(crate) async fn handle_get_version_history(
        &self,
        request: Request<VersionHistoryRequest>,
    ) -> Result<Response<VersionHistoryResponse>, Status> {
        let req = request.into_inner();

        let storage = self.app_state.server.get_storage();
        let storage_read = storage.read();

        let history = if req.is_edge {
            storage_read
                .get_edge_version_history(&req.space, &req.label)
                .map_err(|e| {
                    Status::internal(format!("Failed to get edge version history: {}", e))
                })?
        } else {
            storage_read
                .get_vertex_version_history(&req.space, &req.label)
                .map_err(|e| {
                    Status::internal(format!("Failed to get vertex version history: {}", e))
                })?
        };

        let versions = history
            .map(|h| {
                h.change_log
                    .get_versions()
                    .iter()
                    .map(|&version| {
                        let version_changes = h
                            .change_log
                            .get_version_changes(version)
                            .cloned()
                            .unwrap_or_default();
                        let timestamp_ms = version_changes
                            .iter()
                            .map(|c| c.timestamp_ms)
                            .max()
                            .unwrap_or(0) as i64;
                        let changes = version_changes
                            .into_iter()
                            .map(|change| PropertyChangeEvent {
                                change_type: format!("{:?}", change.details),
                                details: {
                                    let mut details = std::collections::HashMap::new();
                                    details.insert(
                                        "description".to_string(),
                                        change.details.description(),
                                    );
                                    details.insert("version".into(), change.version.to_string());
                                    details
                                },
                            })
                            .collect();

                        SchemaVersion {
                            version,
                            timestamp_ms,
                            changes,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Response::new(VersionHistoryResponse {
            versions,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_get_schema_changes(
        &self,
        request: Request<SchemaChangesRequest>,
    ) -> Result<Response<SchemaChangesResponse>, Status> {
        let req = request.into_inner();

        if req.from_version > req.to_version {
            return Err(Status::invalid_argument(format!(
                "Invalid version range: from_version ({}) must be <= to_version ({})",
                req.from_version, req.to_version
            )));
        }

        let storage = self.app_state.server.get_storage();
        let storage_read = storage.read();

        let changes = if req.is_edge {
            storage_read
                .get_edge_schema_changes(&req.space, &req.label, req.from_version, req.to_version)
                .map_err(|e| {
                    Status::internal(format!("Failed to get edge schema changes: {}", e))
                })?
        } else {
            storage_read
                .get_vertex_schema_changes(&req.space, &req.label, req.from_version, req.to_version)
                .map_err(|e| {
                    Status::internal(format!("Failed to get vertex schema changes: {}", e))
                })?
        };

        let proto_changes = changes
            .iter()
            .map(|change| PropertyChangeEvent {
                change_type: format!("{:?}", change.details),
                details: {
                    let mut details = std::collections::HashMap::new();
                    details.insert("description".into(), change.details.description());
                    details.insert("version".into(), change.version.to_string());
                    details
                },
            })
            .collect();

        Ok(Response::new(SchemaChangesResponse {
            changes: proto_changes,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_detect_breaking_changes(
        &self,
        request: Request<BreakingChangesRequest>,
    ) -> Result<Response<BreakingChangesResponse>, Status> {
        let req = request.into_inner();

        if req.from_version > req.to_version {
            return Err(Status::invalid_argument(format!(
                "Invalid version range: from_version ({}) must be <= to_version ({})",
                req.from_version, req.to_version
            )));
        }

        let storage = self.app_state.server.get_storage();
        let storage_read = storage.read();

        let changes = if req.is_edge {
            storage_read
                .detect_edge_breaking_changes(
                    &req.space,
                    &req.label,
                    req.from_version,
                    req.to_version,
                )
                .map_err(|e| {
                    Status::internal(format!("Failed to detect edge breaking changes: {}", e))
                })?
        } else {
            storage_read
                .detect_vertex_breaking_changes(
                    &req.space,
                    &req.label,
                    req.from_version,
                    req.to_version,
                )
                .map_err(|e| {
                    Status::internal(format!("Failed to detect vertex breaking changes: {}", e))
                })?
        };

        let has_breaking = !changes.is_empty();
        let proto_changes: Vec<PropertyChangeEvent> = changes
            .iter()
            .map(|change| PropertyChangeEvent {
                change_type: format!("{:?}", change.details),
                details: {
                    let mut details = std::collections::HashMap::new();
                    details.insert("description".into(), change.details.description());
                    details.insert("version".into(), change.version.to_string());
                    details
                },
            })
            .collect();

        let recommendation = if has_breaking {
            format!(
                "Found {} breaking changes. Data migration may be required.",
                proto_changes.len()
            )
        } else {
            "No breaking changes detected".to_string()
        };

        Ok(Response::new(BreakingChangesResponse {
            has_breaking_changes: has_breaking,
            changes: proto_changes,
            recommendation,
            error: String::new(),
        }))
    }
}

pub(crate) fn core_space_to_proto(info: &linkrs_core::types::SpaceInfo) -> super::proto::SpaceInfo {
    super::proto::SpaceInfo {
        id: info.space_id as i32,
        name: info.space_name.clone(),
        options: Some(super::proto::SpaceOptions {
            partition_num: info.partition_num,
            replica_num: info.replica_factor,
            charset: String::new(),
            collate: String::new(),
            vid_fixed_length: false,
            vid_length: 0,
        }),
        created_at: 0,
    }
}

pub(crate) fn core_tag_to_proto(info: &linkrs_core::types::TagInfo) -> super::proto::TagInfo {
    super::proto::TagInfo {
        id: info.tag_id as i32,
        name: info.tag_name.clone(),
        properties: info.properties.iter().map(core_property_to_proto).collect(),
        options: Some(super::proto::TagOptions {
            ttl_seconds: info.ttl_duration.unwrap_or(0),
            ttl_column: info.ttl_col.clone().unwrap_or_default(),
        }),
        created_at: 0,
    }
}

pub(crate) fn core_edge_info_to_proto(
    info: &linkrs_core::types::EdgeTypeInfo,
) -> super::proto::EdgeTypeInfo {
    super::proto::EdgeTypeInfo {
        id: info.edge_type_id as i32,
        name: info.edge_type_name.clone(),
        properties: info.properties.iter().map(core_property_to_proto).collect(),
        options: Some(super::proto::EdgeTypeOptions {
            directed: true,
            ttl_seconds: info.ttl_duration.unwrap_or(0),
            ttl_column: info.ttl_col.clone().unwrap_or_default(),
        }),
        created_at: 0,
    }
}

pub(crate) fn proto_property_type_to_data_type(value: i32) -> linkrs_core::DataType {
    use linkrs_core::DataType;
    match value {
        0 => DataType::Bool,
        1 => DataType::Int,
        2 => DataType::Float,
        3 => DataType::Double,
        4 => DataType::String,
        5 | 7 => DataType::DateTime,
        6 => DataType::Date,
        8 => DataType::String,
        9 => DataType::Edge,
        10 => DataType::Vertex,
        11 => DataType::List(Box::new(DataType::Empty)),
        12 => DataType::Set(Box::new(DataType::Empty)),
        13 => DataType::Map(Box::new(DataType::Empty)),
        _ => DataType::String,
    }
}

pub(crate) fn data_type_to_proto_property_type(data_type: &linkrs_core::DataType) -> i32 {
    use linkrs_core::DataType;
    match data_type {
        DataType::Bool => 0,
        DataType::Int | DataType::SmallInt | DataType::BigInt => 1,
        DataType::Float => 2,
        DataType::Double => 3,
        DataType::String | DataType::FixedString(_) => 4,
        DataType::DateTime => 7,
        DataType::Date => 6,
        DataType::Time => 5,
        DataType::Edge => 9,
        DataType::Vertex => 10,
        DataType::List(_) => 11,
        DataType::Set(_) => 12,
        DataType::Map(_) => 13,
        _ => 4,
    }
}

pub(crate) fn proto_property_to_core(
    prop: super::proto::PropertyDef,
) -> linkrs_core::types::PropertyDef {
    linkrs_core::types::PropertyDef::new(prop.name, proto_property_type_to_data_type(prop.r#type))
        .with_nullable(prop.nullable)
}

pub(crate) fn core_property_to_proto(
    prop: &linkrs_core::types::PropertyDef,
) -> super::proto::PropertyDef {
    super::proto::PropertyDef {
        name: prop.name.clone(),
        r#type: data_type_to_proto_property_type(&prop.data_type),
        nullable: prop.nullable,
        default_value: None,
        is_primary_key: false,
    }
}
