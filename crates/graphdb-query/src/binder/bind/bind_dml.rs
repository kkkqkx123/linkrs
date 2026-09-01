use crate::parser::ast::{Assignment, DeleteTarget, InsertTarget, SetOperationType, UpdateTarget};
use graphdb_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_pipe(
        &mut self,
        stmt: &crate::parser::ast::PipeStmt,
    ) -> DBResult<BoundStatement> {
        let statements = vec![self.bind_stmt(&stmt.left)?, self.bind_stmt(&stmt.right)?];

        Ok(BoundStatement::Pipe(BoundPipeStatement {
            span: stmt.span,
            statements,
        }))
    }

    pub(crate) fn bind_set_operation(
        &mut self,
        stmt: &crate::parser::ast::SetOperationStmt,
    ) -> DBResult<BoundStatement> {
        let left = Box::new(self.bind_stmt(&stmt.left)?);
        let right = Box::new(self.bind_stmt(&stmt.right)?);
        let operation = match stmt.op_type {
            SetOperationType::Union | SetOperationType::UnionAll => SetOperationKind::Union,
            SetOperationType::Intersect => SetOperationKind::Intersect,
            SetOperationType::Minus => SetOperationKind::Minus,
        };
        Ok(BoundStatement::SetOperation(BoundSetOperationStatement {
            span: stmt.span,
            left,
            right,
            operation,
        }))
    }

    pub(crate) fn bind_group_by(
        &mut self,
        stmt: &crate::parser::ast::GroupByStmt,
    ) -> DBResult<BoundStatement> {
        let keys = stmt
            .group_items
            .iter()
            .map(|k| self.bind_expr(k))
            .collect::<DBResult<Vec<_>>>()?;

        Ok(BoundStatement::GroupBy(BoundGroupByStatement {
            span: stmt.span,
            keys,
            aggregates: Vec::new(),
        }))
    }

    pub(crate) fn bind_insert(
        &mut self,
        stmt: &crate::parser::ast::InsertStmt,
    ) -> DBResult<BoundStatement> {
        let target = match &stmt.target {
            InsertTarget::Vertices { tags, values } => {
                let mut bound_values = Vec::with_capacity(values.len());
                for row in values {
                    let vid = self.bind_expr(&row.vid)?;
                    let mut tag_values = Vec::with_capacity(row.tag_values.len());
                    for vals in &row.tag_values {
                        let bound_vals = vals
                            .iter()
                            .map(|v| self.bind_expr(v))
                            .collect::<DBResult<Vec<_>>>()?;
                        tag_values.push(bound_vals);
                    }
                    bound_values.push(BoundVertexRow { vid, tag_values });
                }
                BoundInsertTarget::Vertices {
                    tags: tags.clone(),
                    values: bound_values,
                }
            }
            InsertTarget::Edge {
                edge_name,
                prop_names,
                edges,
            } => {
                let mut bound_edges = Vec::with_capacity(edges.len());
                for (src, dst, rank, props) in edges {
                    let src_b = self.bind_expr(src)?;
                    let dst_b = self.bind_expr(dst)?;
                    let rank_b = rank.as_ref().map(|r| self.bind_expr(r)).transpose()?;
                    let props_b = props
                        .iter()
                        .map(|p| self.bind_expr(p))
                        .collect::<DBResult<Vec<_>>>()?;
                    bound_edges.push((src_b, dst_b, rank_b, props_b));
                }
                BoundInsertTarget::Edge {
                    edge_name: edge_name.clone(),
                    prop_names: prop_names.clone(),
                    edges: bound_edges,
                }
            }
        };
        Ok(BoundStatement::Insert(BoundInsert {
            span: stmt.span,
            target,
            if_not_exists: stmt.if_not_exists,
        }))
    }

    pub(crate) fn bind_update(
        &mut self,
        stmt: &crate::parser::ast::UpdateStmt,
    ) -> DBResult<BoundStatement> {
        let target = match &stmt.target {
            UpdateTarget::Vertex(expr) => {
                let b = self.bind_expr(expr)?;
                BoundUpdateTarget::Vertex(b)
            }
            UpdateTarget::Edge {
                src,
                dst,
                edge_type,
                rank,
            } => {
                let src_b = self.bind_expr(src)?;
                let dst_b = self.bind_expr(dst)?;
                let rank_b = rank.as_ref().map(|r| self.bind_expr(r)).transpose()?;
                BoundUpdateTarget::Edge {
                    src: src_b,
                    dst: dst_b,
                    edge_type: edge_type.clone(),
                    rank: rank_b,
                }
            }
            UpdateTarget::Tag(tag) => BoundUpdateTarget::Tag(tag.clone()),
            UpdateTarget::TagOnVertex { vid, tag_name } => {
                let vid_b = self.bind_expr(vid)?;
                BoundUpdateTarget::TagOnVertex {
                    vid: vid_b,
                    tag_name: tag_name.clone(),
                }
            }
        };
        let assignments = Self::bind_assignments(self, &stmt.set_clause.assignments)?;
        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|w| self.bind_expr(w))
            .transpose()?;
        Ok(BoundStatement::Update(BoundUpdate {
            span: stmt.span,
            target,
            assignments,
            where_clause,
            is_upsert: stmt.is_upsert,
        }))
    }

    pub(crate) fn bind_delete(
        &mut self,
        stmt: &crate::parser::ast::DeleteStmt,
    ) -> DBResult<BoundStatement> {
        let target = match &stmt.target {
            DeleteTarget::Vertices(exprs) => {
                let vals = exprs
                    .iter()
                    .map(|e| self.bind_expr(e))
                    .collect::<DBResult<Vec<_>>>()?;
                BoundDeleteTarget::Vertices(vals)
            }
            DeleteTarget::Edges { edge_type, edges } => {
                let mut bound = Vec::with_capacity(edges.len());
                for (src, dst, rank) in edges {
                    let s = self.bind_expr(src)?;
                    let d = self.bind_expr(dst)?;
                    let r = rank.as_ref().map(|v| self.bind_expr(v)).transpose()?;
                    bound.push((s, d, r));
                }
                BoundDeleteTarget::Edges {
                    edge_type: edge_type.clone(),
                    edges: bound,
                }
            }
            DeleteTarget::Tags {
                tag_names,
                vertex_ids,
                is_all_tags,
            } => {
                let vids = vertex_ids
                    .iter()
                    .map(|v| self.bind_expr(v))
                    .collect::<DBResult<Vec<_>>>()?;
                BoundDeleteTarget::Tags {
                    tag_names: tag_names.clone(),
                    vertex_ids: vids,
                    is_all_tags: *is_all_tags,
                }
            }
            DeleteTarget::Index(name) => BoundDeleteTarget::Index(name.clone()),
        };
        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|w| self.bind_expr(w))
            .transpose()?;
        Ok(BoundStatement::Delete(BoundDelete {
            span: stmt.span,
            target,
            where_clause,
            with_edge: stmt.with_edge,
        }))
    }

    pub(crate) fn bind_merge(
        &mut self,
        stmt: &crate::parser::ast::MergeStmt,
    ) -> DBResult<BoundStatement> {
        let on_create = if let Some(clause) = &stmt.on_create {
            Self::bind_assignments(self, &clause.assignments)?
        } else {
            Vec::new()
        };
        let on_match = if let Some(clause) = &stmt.on_match {
            Self::bind_assignments(self, &clause.assignments)?
        } else {
            Vec::new()
        };
        // Validate pattern: MERGE requires a node or path pattern. No additional
        // emptiness check needed because `Pattern` is an enum (Node/Edge/Path/Variable)
        // and the parser guarantees a non-empty pattern.
        Ok(BoundStatement::Merge(BoundMerge {
            span: stmt.span,
            pattern: stmt.pattern.clone(),
            on_create,
            on_match,
        }))
    }

    pub(crate) fn bind_set(
        &mut self,
        stmt: &crate::parser::ast::SetStmt,
    ) -> DBResult<BoundStatement> {
        let assignments = Self::bind_assignments(self, &stmt.assignments)?;
        Ok(BoundStatement::Set(BoundSet {
            span: stmt.span,
            assignments,
        }))
    }

    pub(crate) fn bind_remove(
        &mut self,
        stmt: &crate::parser::ast::RemoveStmt,
    ) -> DBResult<BoundStatement> {
        let items = stmt
            .items
            .iter()
            .map(|e| self.bind_expr(e))
            .collect::<DBResult<Vec<_>>>()?;
        Ok(BoundStatement::Remove(BoundRemove {
            span: stmt.span,
            items,
        }))
    }

    pub(crate) fn bind_copy(
        &mut self,
        stmt: &crate::parser::ast::CopyStmt,
    ) -> DBResult<BoundStatement> {
        if stmt.file_path.is_empty() {
            return Err(graphdb_core::error::DBError::from(
                graphdb_core::error::QueryError::invalid_query(
                    "COPY file path cannot be empty".to_string(),
                ),
            ));
        }
        Ok(BoundStatement::Copy(BoundCopy {
            span: stmt.span,
            target: stmt.target.clone(),
            direction: stmt.direction,
            file_path: stmt.file_path.clone(),
            header: stmt.header,
            delimiter: stmt.delimiter,
            batch_size: stmt.batch_size,
        }))
    }

    fn bind_assignments(&mut self, assignments: &[Assignment]) -> DBResult<Vec<BoundAssignment>> {
        let mut out = Vec::with_capacity(assignments.len());
        for a in assignments {
            let value = self.bind_expr(&a.value)?;
            let target = a.target.as_ref().map(|t| self.bind_expr(t)).transpose()?;
            let object = a.object.as_ref().map(|o| self.bind_expr(o)).transpose()?;
            out.push(BoundAssignment {
                property: a.property.clone(),
                value,
                target,
                object,
            });
        }
        Ok(out)
    }
}
