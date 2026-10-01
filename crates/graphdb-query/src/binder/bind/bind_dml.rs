use crate::parser::ast::pattern::{NodePattern, Pattern};
use crate::parser::ast::{Assignment, DeleteTarget, InsertTarget, SetOperationType, UpdateTarget};
use graphdb_core::types::Expression;
use graphdb_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_pipe(
        &mut self,
        stmt: &crate::parser::ast::PipeStmt,
    ) -> DBResult<BoundStatement> {
        let statements = vec![self.bind_stmt(&stmt.left)?, self.bind_stmt(&stmt.right)?];

        Ok(BoundStatement::Pipe(BoundPipeStatement { statements }))
    }

    pub(crate) fn bind_set_operation(
        &mut self,
        stmt: &crate::parser::ast::SetOperationStmt,
    ) -> DBResult<BoundStatement> {
        let left = Box::new(self.bind_stmt(&stmt.left)?);
        let right = Box::new(self.bind_stmt(&stmt.right)?);
        let operation = match stmt.op_type {
            SetOperationType::Union => SetOperationKind::Union,
            SetOperationType::UnionAll => SetOperationKind::UnionAll,
            SetOperationType::Intersect => SetOperationKind::Intersect,
            SetOperationType::Minus => SetOperationKind::Minus,
        };
        Ok(BoundStatement::SetOperation(BoundSetOperationStatement {
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

        let mut aggregates = Vec::new();
        for item in &stmt.yield_clause.items {
            let bound = self.bind_expr(&item.expression)?;
            Self::collect_bound_aggregates(&bound, item.alias.clone(), &mut aggregates);
        }

        Ok(BoundStatement::GroupBy(BoundGroupByStatement {
            keys,
            aggregates,
        }))
    }

    fn collect_bound_aggregates(
        bound: &BoundExpression,
        alias: Option<String>,
        out: &mut Vec<crate::binder::bound::BoundAggregateCall>,
    ) {
        match bound {
            BoundExpression::Aggregate(agg) => {
                let mut agg = agg.clone();
                if agg.alias.is_none() {
                    agg.alias = alias;
                }
                out.push(agg);
            }
            BoundExpression::BinaryOp { left, right, .. } => {
                Self::collect_bound_aggregates(left, None, out);
                Self::collect_bound_aggregates(right, None, out);
            }
            BoundExpression::UnaryOp { operand, .. } => {
                Self::collect_bound_aggregates(operand, None, out);
            }
            BoundExpression::Function(f) => {
                for arg in &f.args {
                    Self::collect_bound_aggregates(arg, None, out);
                }
            }
            BoundExpression::Cast { expr, .. } => {
                Self::collect_bound_aggregates(expr, None, out);
            }
            BoundExpression::List(items, _) => {
                for item in items {
                    Self::collect_bound_aggregates(item, None, out);
                }
            }
            BoundExpression::Map(pairs, _) => {
                for (_, v) in pairs {
                    Self::collect_bound_aggregates(v, None, out);
                }
            }
            BoundExpression::Case {
                expr,
                when_then,
                else_expr,
                ..
            } => {
                if let Some(e) = expr {
                    Self::collect_bound_aggregates(e, None, out);
                }
                for (c, v) in when_then {
                    Self::collect_bound_aggregates(c, None, out);
                    Self::collect_bound_aggregates(v, None, out);
                }
                if let Some(e) = else_expr {
                    Self::collect_bound_aggregates(e, None, out);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn bind_insert(
        &mut self,
        stmt: &crate::parser::ast::InsertStmt,
    ) -> DBResult<BoundStatement> {
        let target = match &stmt.target {
            InsertTarget::Vertices { tag, values } => {
                let mut bound_values = Vec::with_capacity(values.len());
                for row in values {
                    let vid = self.bind_expr(&row.vid)?;
                    let bound_vals = row
                        .values
                        .iter()
                        .map(|v| self.bind_expr(v))
                        .collect::<DBResult<Vec<_>>>()?;
                    bound_values.push(BoundVertexRow {
                        vid,
                        values: bound_vals,
                    });
                }
                BoundInsertTarget::Vertices {
                    tag: tag.clone(),
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
            target,
            if_not_exists: stmt.if_not_exists,
        }))
    }

    pub(crate) fn bind_update(
        &mut self,
        stmt: &crate::parser::ast::UpdateStmt,
    ) -> DBResult<BoundStatement> {
        reject_map_overwrite(&stmt.set_clause.assignments)?;
        let target_alias = update_target_alias(&stmt.target);
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
                BoundUpdateTarget::Edge(Box::new(crate::binder::bound::BoundEdgeUpdateTarget {
                    src: src_b,
                    dst: dst_b,
                    edge_type: edge_type.clone(),
                    rank: rank_b,
                }))
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
        let assignments =
            Self::bind_assignments(self, &stmt.set_clause.assignments, target_alias.as_deref())?;
        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|w| self.bind_expr(w))
            .transpose()?
            .map(|bound| match target_alias.as_deref() {
                Some(alias) if self.scope.lookup(alias).is_none() => {
                    normalize_target_alias_refs(bound, alias)
                }
                _ => bound,
            });
        Ok(BoundStatement::Update(BoundUpdate {
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
            DeleteTarget::Vertices { tag, vids } => {
                let vals = vids
                    .iter()
                    .map(|e| self.bind_expr(e))
                    .collect::<DBResult<Vec<_>>>()?;
                BoundDeleteTarget::Vertices {
                    tag: tag.clone(),
                    ids: vals,
                }
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
            DeleteTarget::Index(name) => BoundDeleteTarget::Index(name.clone()),
        };
        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|w| self.bind_expr(w))
            .transpose()?;
        Ok(BoundStatement::Delete(BoundDelete {
            target,
            where_clause,
            with_edge: stmt.with_edge,
            detach: stmt.detach,
        }))
    }

    pub(crate) fn bind_merge(
        &mut self,
        stmt: &crate::parser::ast::MergeStmt,
    ) -> DBResult<BoundStatement> {
        if let Some(clause) = &stmt.on_create {
            reject_map_overwrite(&clause.assignments)?;
        }
        if let Some(clause) = &stmt.on_match {
            reject_map_overwrite(&clause.assignments)?;
        }
        let target_alias = match &stmt.pattern {
            Pattern::Node(node) => node.variable.clone(),
            _ => None,
        };
        let on_create = if let Some(clause) = &stmt.on_create {
            Self::bind_assignments(self, &clause.assignments, target_alias.as_deref())?
        } else {
            Vec::new()
        };
        let on_match = if let Some(clause) = &stmt.on_match {
            Self::bind_assignments(self, &clause.assignments, target_alias.as_deref())?
        } else {
            Vec::new()
        };
        let bound_pattern = self.bind_merge_pattern(&stmt.pattern)?;
        Ok(BoundStatement::Merge(BoundMerge {
            pattern: bound_pattern,
            on_create,
            on_match,
        }))
    }

    pub(crate) fn bind_set(
        &mut self,
        stmt: &crate::parser::ast::SetStmt,
    ) -> DBResult<BoundStatement> {
        let assignments = Self::bind_assignments(self, &stmt.assignments, None)?;
        Ok(BoundStatement::Set(BoundSet { assignments }))
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
        Ok(BoundStatement::Remove(BoundRemove { items }))
    }

    pub(crate) fn bind_copy(
        &mut self,
        stmt: &crate::parser::ast::CopyStmt,
    ) -> DBResult<BoundStatement> {
        if stmt.file_paths.is_empty() || stmt.file_paths.iter().any(|p| p.trim().is_empty()) {
            return Err(graphdb_core::error::DBError::from(
                graphdb_core::error::QueryError::invalid_query(
                    "COPY file path cannot be empty".to_string(),
                ),
            ));
        }
        if stmt.direction == crate::parser::ast::CopyDirection::To && stmt.file_paths.len() > 1 {
            return Err(graphdb_core::error::DBError::from(
                graphdb_core::error::QueryError::invalid_query(
                    "COPY TO supports a single file path".to_string(),
                ),
            ));
        }
        Ok(BoundStatement::Copy(BoundCopy {
            target: stmt.target.clone(),
            direction: stmt.direction,
            file_paths: stmt.file_paths.clone(),
            by_column: stmt.by_column,
            header: stmt.header,
            delimiter: stmt.delimiter,
            batch_size: stmt.batch_size,
        }))
    }

    fn bind_assignments(
        &mut self,
        assignments: &[Assignment],
        target_alias: Option<&str>,
    ) -> DBResult<Vec<BoundAssignment>> {
        let normalize = match target_alias {
            Some(alias) if self.scope.lookup(alias).is_none() => Some(alias),
            _ => None,
        };
        let mut out = Vec::with_capacity(assignments.len());
        for a in assignments {
            let value = self.bind_expr(&a.value)?;
            let value = match normalize {
                Some(alias) => normalize_target_alias_refs(value, alias),
                None => value,
            };
            let target = a.target.as_ref().map(|t| self.bind_expr(t)).transpose()?;
            let object = a.object.as_ref().map(|o| self.bind_expr(o)).transpose()?;
            out.push(BoundAssignment {
                property: a.property.clone(),
                value,
                target,
                object,
                is_map_overwrite: a.is_map_overwrite,
            });
        }
        Ok(out)
    }

    fn bind_merge_pattern(&mut self, pattern: &Pattern) -> DBResult<BoundMergePattern> {
        match pattern {
            Pattern::Node(np) => Ok(BoundMergePattern::Node(self.bind_pattern_vertex(np)?)),
            Pattern::Edge(ep) => {
                let edge = self.bind_pattern_edge(ep)?;
                let src = BoundPatternVertex {
                    variable: None,
                    labels: Vec::new(),
                    properties: None,
                };
                let dst = BoundPatternVertex {
                    variable: None,
                    labels: Vec::new(),
                    properties: None,
                };
                Ok(BoundMergePattern::Edge { src, edge, dst })
            }
            _ => Err(graphdb_core::error::DBError::from(
                graphdb_core::error::QueryError::invalid_query(
                    "MERGE only supports node or edge patterns".to_string(),
                ),
            )),
        }
    }

    fn bind_pattern_vertex(&mut self, np: &NodePattern) -> DBResult<BoundPatternVertex> {
        let properties = np
            .properties
            .as_ref()
            .map(|p| self.extract_map_properties(p))
            .transpose()?;
        Ok(BoundPatternVertex {
            variable: np.variable.clone(),
            labels: np.labels.clone(),
            properties,
        })
    }

    fn bind_pattern_edge(
        &mut self,
        ep: &crate::parser::ast::pattern::EdgePattern,
    ) -> DBResult<BoundPatternEdge> {
        let properties = ep
            .properties
            .as_ref()
            .map(|p| self.extract_map_properties(p))
            .transpose()?;
        Ok(BoundPatternEdge {
            variable: ep.variable.clone(),
            edge_types: ep.edge_types.clone(),
            properties,
            direction: ep.direction,
        })
    }

    pub(crate) fn extract_map_properties(
        &self,
        expr: &graphdb_core::types::ContextualExpression,
    ) -> DBResult<Vec<(String, BoundExpression)>> {
        use graphdb_core::types::Expression;
        if let Some(meta) = expr.expression() {
            if let Expression::Map(pairs) = meta.inner() {
                let mut result = Vec::with_capacity(pairs.len());
                for (key, val) in pairs {
                    let bound_val = self.convert_ast_expr_to_bound(val)?;
                    result.push((key.clone(), bound_val));
                }
                return Ok(result);
            }
        }
        Ok(Vec::new())
    }

    pub(crate) fn convert_ast_expr_to_bound(&self, expr: &Expression) -> DBResult<BoundExpression> {
        use graphdb_core::types::expr::contextual::ContextualExpression;
        let ctx = std::sync::Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let meta = graphdb_core::types::expr::ExpressionMeta::new(expr.clone());
        let id = ctx.register_expression(meta);
        let contextual = ContextualExpression::new(id, ctx);
        let mut binder = super::Binder::new();
        binder.scope = self.scope.clone();
        binder.schema_manager = self.schema_manager.clone();
        binder.space_name = self.space_name.clone();
        binder.bind_expr(&contextual)
    }
}

/// Reject whole-map overwrite assignments (`SET v = {...}`).
///
/// The storage layer only merges per-property writes, so accepting the
/// map form would silently keep stale properties. Fail explicitly at bind
/// time instead of mis-executing.
fn reject_map_overwrite(assignments: &[Assignment]) -> DBResult<()> {
    if assignments.iter().any(|a| a.is_map_overwrite) {
        return Err(graphdb_core::error::DBError::from(
            graphdb_core::error::QueryError::invalid_query(
                "Whole-map overwrite (SET v = {...}) is not supported: assign properties individually"
                    .to_string(),
            ),
        ));
    }
    Ok(())
}

/// The variable alias naming an update target, when the target carries one.
///
/// Only single-alias vertex targets qualify: edge targets name endpoints
/// rather than the updated row, so they are left out deliberately.
fn update_target_alias(target: &UpdateTarget) -> Option<String> {
    match target {
        UpdateTarget::TagOnVertex { vid, .. } => variable_name(vid),
        UpdateTarget::Vertex(expr) => variable_name(expr),
        _ => None,
    }
}

/// The variable name carried by an expression, if it is a bare variable.
fn variable_name(
    expr: &graphdb_core::types::expr::contextual::ContextualExpression,
) -> Option<String> {
    match expr.expression().map(|meta| meta.inner().clone()) {
        Some(Expression::Variable(name)) => Some(name),
        _ => None,
    }
}

/// Rewrite `alias.prop` references to bare property names.
///
/// Update sinks resolve bare names against the target row's existing
/// properties, while the target alias itself is never bound in the row
/// context. The rewrite is semantics-preserving exactly there, so callers
/// must only apply it when the alias is otherwise unbound in scope.
/// References under any other qualifier, and every subquery body, are left
/// untouched.
fn normalize_target_alias_refs(expr: BoundExpression, alias: &str) -> BoundExpression {
    match expr {
        BoundExpression::Property {
            object,
            property,
            value_type,
        } => match *object {
            BoundExpression::Variable(name, _) if name == alias => {
                BoundExpression::Variable(property, value_type)
            }
            other => BoundExpression::Property {
                object: Box::new(normalize_target_alias_refs(other, alias)),
                property,
                value_type,
            },
        },
        BoundExpression::StructField {
            base,
            field,
            return_type,
        } => BoundExpression::StructField {
            base: Box::new(normalize_target_alias_refs(*base, alias)),
            field,
            return_type,
        },
        BoundExpression::BinaryOp {
            left,
            op,
            right,
            return_type,
        } => BoundExpression::BinaryOp {
            left: Box::new(normalize_target_alias_refs(*left, alias)),
            op,
            right: Box::new(normalize_target_alias_refs(*right, alias)),
            return_type,
        },
        BoundExpression::UnaryOp {
            op,
            operand,
            return_type,
        } => BoundExpression::UnaryOp {
            op,
            operand: Box::new(normalize_target_alias_refs(*operand, alias)),
            return_type,
        },
        BoundExpression::Function(mut call) => {
            call.args = call
                .args
                .into_iter()
                .map(|arg| normalize_target_alias_refs(arg, alias))
                .collect();
            BoundExpression::Function(call)
        }
        BoundExpression::Aggregate(mut call) => {
            call.arguments = call
                .arguments
                .into_iter()
                .map(|arg| normalize_target_alias_refs(arg, alias))
                .collect();
            BoundExpression::Aggregate(call)
        }
        BoundExpression::List(items, data_type) => BoundExpression::List(
            items
                .into_iter()
                .map(|item| normalize_target_alias_refs(item, alias))
                .collect(),
            data_type,
        ),
        BoundExpression::Map(pairs, data_type) => BoundExpression::Map(
            pairs
                .into_iter()
                .map(|(name, value)| (name, normalize_target_alias_refs(value, alias)))
                .collect(),
            data_type,
        ),
        BoundExpression::Case {
            expr,
            when_then,
            else_expr,
            return_type,
        } => BoundExpression::Case {
            expr: expr.map(|inner| Box::new(normalize_target_alias_refs(*inner, alias))),
            when_then: when_then
                .into_iter()
                .map(|(when, then)| {
                    (
                        normalize_target_alias_refs(when, alias),
                        normalize_target_alias_refs(then, alias),
                    )
                })
                .collect(),
            else_expr: else_expr.map(|inner| Box::new(normalize_target_alias_refs(*inner, alias))),
            return_type,
        },
        BoundExpression::Cast { expr, target_type } => BoundExpression::Cast {
            expr: Box::new(normalize_target_alias_refs(*expr, alias)),
            target_type,
        },
        BoundExpression::Predicate {
            func,
            args,
            return_type,
        } => BoundExpression::Predicate {
            func,
            args: args
                .into_iter()
                .map(|arg| normalize_target_alias_refs(arg, alias))
                .collect(),
            return_type,
        },
        BoundExpression::Subscript {
            collection,
            index,
            return_type,
        } => BoundExpression::Subscript {
            collection: Box::new(normalize_target_alias_refs(*collection, alias)),
            index: Box::new(normalize_target_alias_refs(*index, alias)),
            return_type,
        },
        BoundExpression::Path(items, data_type) => BoundExpression::Path(
            items
                .into_iter()
                .map(|item| normalize_target_alias_refs(item, alias))
                .collect(),
            data_type,
        ),
        BoundExpression::PathBuild(items, data_type) => BoundExpression::PathBuild(
            items
                .into_iter()
                .map(|item| normalize_target_alias_refs(item, alias))
                .collect(),
            data_type,
        ),
        BoundExpression::ListComprehension {
            variable,
            source,
            filter,
            map,
            return_type,
        } => {
            if variable == alias {
                BoundExpression::ListComprehension {
                    variable,
                    source,
                    filter,
                    map,
                    return_type,
                }
            } else {
                BoundExpression::ListComprehension {
                    variable,
                    source: Box::new(normalize_target_alias_refs(*source, alias)),
                    filter: filter
                        .map(|inner| Box::new(normalize_target_alias_refs(*inner, alias))),
                    map: map.map(|inner| Box::new(normalize_target_alias_refs(*inner, alias))),
                    return_type,
                }
            }
        }
        BoundExpression::Reduce {
            accumulator,
            initial,
            variable,
            source,
            mapping,
            return_type,
        } => {
            if accumulator == alias || variable == alias {
                BoundExpression::Reduce {
                    accumulator,
                    initial,
                    variable,
                    source,
                    mapping,
                    return_type,
                }
            } else {
                BoundExpression::Reduce {
                    accumulator,
                    initial: Box::new(normalize_target_alias_refs(*initial, alias)),
                    variable,
                    source: Box::new(normalize_target_alias_refs(*source, alias)),
                    mapping: Box::new(normalize_target_alias_refs(*mapping, alias)),
                    return_type,
                }
            }
        }
        BoundExpression::Lambda { params, body } => {
            if params.iter().any(|param| param == alias) {
                BoundExpression::Lambda { params, body }
            } else {
                BoundExpression::Lambda {
                    params,
                    body: Box::new(normalize_target_alias_refs(*body, alias)),
                }
            }
        }
        BoundExpression::In {
            expr,
            subquery,
            negated,
            original_body,
        } => BoundExpression::In {
            expr: Box::new(normalize_target_alias_refs(*expr, alias)),
            subquery,
            negated,
            original_body,
        },
        other => other,
    }
}
