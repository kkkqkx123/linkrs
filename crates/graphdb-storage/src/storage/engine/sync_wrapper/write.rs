use super::SyncWrapper;
use crate::core::types::{InsertEdgeInfo, InsertVertexInfo, UpdateInfo, UpdateOp, VertexId};
use crate::core::{Edge, StorageError, Value, Vertex};
use crate::storage::{StorageClient, StorageWriter};
use crate::sync::types::ChangeType;

impl<S: StorageClient + 'static> SyncWrapper<S> {
    fn reject_staged_write(&self, error: StorageError) -> StorageError {
        if let Some(transaction_id) = self.get_current_txn_id() {
            let _ = self.abort_transaction_fact(transaction_id);
        }
        error
    }

    fn stage_vertex_data_change(
        &self,
        info: &InsertVertexInfo,
        change_type: ChangeType,
    ) -> Result<(), StorageError> {
        if !self.enabled {
            return Ok(());
        }
        let Some(manager) = self.get_sync_manager() else {
            return Ok(());
        };
        let transaction_id = self.get_current_txn_id().ok_or_else(|| {
            StorageError::db_error(
                "Synchronized writes require an operation transaction context".to_string(),
            )
        })?;
        manager
            .on_vertex_change_with_txn(
                transaction_id,
                info.space_id,
                &info.tag_name,
                &info.vertex_id,
                &info.props,
                change_type,
            )
            .map_err(|error| StorageError::db_error(error.to_string()))
    }
}

impl<S: StorageClient + 'static> StorageWriter for SyncWrapper<S> {
    fn insert_vertex(&mut self, space: &str, vertex: Vertex) -> Result<VertexId, StorageError> {
        let result = self.inner.insert_vertex(space, vertex.clone())?;
        if let Err(error) = self.sync_insert_vertex(space, &vertex) {
            return Err(self.reject_staged_write(error));
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn update_vertex(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError> {
        let old_vertex = self
            .inner
            .get_vertex(space, &vertex.vid)?
            .ok_or_else(|| StorageError::node_not_found(vertex.vid))?;

        self.inner.update_vertex(space, vertex.clone())?;
        if let Err(error) = self.sync_update_vertex(space, &old_vertex, &vertex) {
            return Err(self.reject_staged_write(error));
        }
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn delete_vertex(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        let vertex = self
            .inner
            .get_vertex(space, id)?
            .ok_or_else(|| StorageError::node_not_found(*id))?;

        StorageWriter::delete_vertex(&mut self.inner, space, id)?;
        if let Err(error) = self.sync_delete_vertex(space, id, &vertex) {
            return Err(self.reject_staged_write(error));
        }
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn delete_vertex_with_edges(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        let vertex = self
            .inner
            .get_vertex(space, id)?
            .ok_or_else(|| StorageError::node_not_found(*id))?;

        StorageWriter::delete_vertex_with_edges(&mut self.inner, space, id)?;
        if let Err(error) = self.sync_delete_vertex(space, id, &vertex) {
            return Err(self.reject_staged_write(error));
        }
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn batch_insert_vertices(
        &mut self,
        space: &str,
        vertices: Vec<Vertex>,
    ) -> Result<Vec<VertexId>, StorageError> {
        let results = self.inner.batch_insert_vertices(space, vertices.clone())?;
        if let Err(error) = self.sync_batch_insert_vertices(space, &vertices) {
            return Err(self.reject_staged_write(error));
        }
        self.commit_auto_transaction()?;
        Ok(results)
    }

    fn delete_tags(
        &mut self,
        space: &str,
        vertex_id: &VertexId,
        tag_names: &[String],
    ) -> Result<usize, StorageError> {
        let previous = self.inner.get_vertex(space, vertex_id)?;
        let result = self.inner.delete_tags(space, vertex_id, tag_names)?;
        if result > 0 && self.enabled {
            if let Some(manager) = self.get_sync_manager() {
                let transaction_id = self
                    .get_current_txn_id()
                    .ok_or_else(|| {
                        StorageError::db_error(
                            "Synchronized writes require an operation transaction context"
                                .to_string(),
                        )
                    })
                    .map_err(|error| self.reject_staged_write(error))?;
                let space_id = self
                    .inner
                    .get_space_id(space)
                    .map_err(|error| self.reject_staged_write(error))?;
                for tag in previous
                    .into_iter()
                    .flat_map(|vertex| vertex.tags)
                    .filter(|tag| tag_names.iter().any(|name| name == &tag.name))
                {
                    let properties = tag.properties.into_iter().collect::<Vec<_>>();
                    if let Err(error) = manager.on_vertex_change_with_txn(
                        transaction_id,
                        space_id,
                        &tag.name,
                        &Value::from(*vertex_id),
                        &properties,
                        ChangeType::Delete,
                    ) {
                        return Err(
                            self.reject_staged_write(StorageError::db_error(error.to_string()))
                        );
                    }
                }
            }
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn insert_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError> {
        let result = self.inner.insert_edge(space, edge.clone());
        if result.is_ok() {
            if let Err(error) = self.sync_insert_edge(space, &edge) {
                return Err(self.reject_staged_write(error));
            }
        }
        result?;
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn update_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError> {
        let result = self.inner.update_edge(space, edge.clone());
        if result.is_ok() {
            if let Err(error) =
                self.sync_delete_edge(space, &edge.src, &edge.dst, &edge.edge_type, edge.ranking)
            {
                return Err(self.reject_staged_write(error));
            }
            if let Err(error) = self.sync_insert_edge(space, &edge) {
                return Err(self.reject_staged_write(error));
            }
        }
        result?;
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn delete_edge(
        &mut self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<(), StorageError> {
        let result = StorageWriter::delete_edge(&mut self.inner, space, src, dst, edge_type, rank);
        if result.is_ok() {
            if let Err(error) = self.sync_delete_edge(space, src, dst, edge_type, rank) {
                return Err(self.reject_staged_write(error));
            }
        }
        result?;
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn batch_insert_edges(&mut self, space: &str, edges: Vec<Edge>) -> Result<(), StorageError> {
        let result = self.inner.batch_insert_edges(space, edges.clone());
        if result.is_ok() {
            if let Err(error) = self.sync_batch_insert_edges(space, &edges) {
                return Err(self.reject_staged_write(error));
            }
        }
        result?;
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn insert_vertex_data(
        &mut self,
        space: &str,
        info: &InsertVertexInfo,
    ) -> Result<bool, StorageError> {
        let result = self.inner.insert_vertex_data(space, info)?;
        if result {
            if let Err(error) = self.stage_vertex_data_change(info, ChangeType::Insert) {
                return Err(self.reject_staged_write(error));
            }
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn delete_vertex_data(&mut self, space: &str, vertex_id: &str) -> Result<bool, StorageError> {
        let parsed_id = vertex_id
            .parse::<i64>()
            .map(VertexId::from_int64)
            .unwrap_or_else(|_| VertexId::from_string(vertex_id));
        let previous = self.inner.get_vertex(space, &parsed_id)?;
        let result = self.inner.delete_vertex_data(space, vertex_id)?;
        if result {
            if let Some(vertex) = previous {
                if let Err(error) = self.sync_delete_vertex(space, &parsed_id, &vertex) {
                    return Err(self.reject_staged_write(error));
                }
            }
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn insert_edge_data(
        &mut self,
        space: &str,
        info: &InsertEdgeInfo,
    ) -> Result<bool, StorageError> {
        let edge = Edge {
            src: VertexId::try_from(&info.src_vertex_id)
                .map_err(|error| StorageError::invalid_input(error.to_string()))?,
            dst: VertexId::try_from(&info.dst_vertex_id)
                .map_err(|error| StorageError::invalid_input(error.to_string()))?,
            edge_type: info.edge_name.clone(),
            ranking: info.rank,
            props: info.props.iter().cloned().collect(),
        };
        let result = self.inner.insert_edge_data(space, info)?;
        if result {
            if let Err(error) = self.sync_insert_edge(space, &edge) {
                return Err(self.reject_staged_write(error));
            }
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn delete_edge_data(
        &mut self,
        space: &str,
        src: &str,
        dst: &str,
        rank: i64,
    ) -> Result<bool, StorageError> {
        let source = src
            .parse::<i64>()
            .map(VertexId::from_int64)
            .unwrap_or_else(|_| VertexId::from_string(src));
        let destination = dst
            .parse::<i64>()
            .map(VertexId::from_int64)
            .unwrap_or_else(|_| VertexId::from_string(dst));
        let previous = self
            .inner
            .scan_all_edges(space)?
            .into_iter()
            .filter(|edge| edge.src == source && edge.dst == destination && edge.ranking == rank)
            .collect::<Vec<_>>();
        let result = self.inner.delete_edge_data(space, src, dst, rank)?;
        if result {
            for edge in previous {
                if let Err(error) = self.sync_delete_edge(
                    space,
                    &edge.src,
                    &edge.dst,
                    &edge.edge_type,
                    edge.ranking,
                ) {
                    return Err(self.reject_staged_write(error));
                }
            }
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn update_data(
        &mut self,
        space: &str,
        space_id: u64,
        info: &UpdateInfo,
    ) -> Result<bool, StorageError> {
        let target = &info.update_target;
        let vertex_id = VertexId::try_from(&target.id)
            .map_err(|error| StorageError::invalid_input(error.to_string()))?;
        let previous = self.inner.get_vertex(space, &vertex_id)?;
        let current = previous
            .as_ref()
            .and_then(|vertex| {
                vertex
                    .tags
                    .iter()
                    .find(|tag| tag.name == target.label)
                    .and_then(|tag| tag.properties.get(&target.prop))
            })
            .cloned();
        let updated_value = match (&info.update_op, current) {
            (UpdateOp::Add, Some(Value::Int(current))) => match &info.value {
                Value::Int(delta) => Value::Int(current + *delta),
                _ => info.value.clone(),
            },
            (UpdateOp::Subtract, Some(Value::Int(current))) => match &info.value {
                Value::Int(delta) => Value::Int(current - *delta),
                _ => info.value.clone(),
            },
            _ => info.value.clone(),
        };
        let result = self.inner.update_data(space, space_id, info)?;
        if result && self.enabled && self.get_sync_manager().is_some() {
            let transaction_id = self
                .get_current_txn_id()
                .ok_or_else(|| {
                    StorageError::db_error(
                        "Synchronized writes require an operation transaction context".to_string(),
                    )
                })
                .map_err(|error| self.reject_staged_write(error))?;
            if let Some(manager) = self.get_sync_manager() {
                if let Err(error) = manager.on_vertex_change_with_txn(
                    transaction_id,
                    space_id,
                    &target.label,
                    &target.id,
                    &[(target.prop.clone(), updated_value)],
                    ChangeType::Update,
                ) {
                    return Err(self.reject_staged_write(StorageError::db_error(error.to_string())));
                }
            }
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }
}
