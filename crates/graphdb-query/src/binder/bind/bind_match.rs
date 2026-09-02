use crate::parser::ast::pattern::{PathElement, Pattern};
use crate::parser::ast::MatchDeleteTarget;
use graphdb_core::error::DBError;
use graphdb_core::types::semantic::{AliasType, ValueType};
use graphdb_core::DBResult;

use crate::binder::bound::*;
use crate::binder::query_graph::*;
use crate::binder::scope::{BinderScope, BinderVariable};

use super::Binder;

impl Binder {
    // ── MATCH binding (produces QueryGraph) ────────────────────────────────

    pub(crate) fn bind_match(
        &mut self,
        stmt: &crate::parser::ast::MatchStmt,
    ) -> DBResult<BoundStatement> {
        let query_graph = self.build_query_graph(&stmt.patterns)?;

        // Register MATCH variables in scope BEFORE binding WHERE / RETURN /
        // ORDER BY so those clauses can reference the matched entities.
        for node in &query_graph.nodes {
            self.scope.define_variable(BinderVariable {
                name: node.variable.clone(),
                alias_type: AliasType::Node,
                tags: node.tags.iter().map(|t| t.tag_name.to_string()).collect(),
                properties: node
                    .tags
                    .iter()
                    .flat_map(|t| t.properties.clone())
                    .collect(),
                is_defined: true,
            });
        }
        for edge in &query_graph.edges {
            self.scope.define_variable(BinderVariable {
                name: edge.variable.clone(),
                alias_type: AliasType::Edge,
                tags: edge
                    .edge_types
                    .iter()
                    .map(|e| e.edge_type_name.to_string())
                    .collect(),
                properties: edge
                    .edge_types
                    .iter()
                    .flat_map(|e| e.properties.clone())
                    .collect(),
                is_defined: true,
            });
        }

        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|c| {
                self.bind_expr(c)
                    .map(|be| BoundWhereClause { condition: be })
            })
            .transpose()?;

        let return_clause = stmt
            .return_clause
            .as_ref()
            .map(|rc| self.bind_return_clause(rc))
            .transpose()?;

        let order_by = stmt
            .order_by
            .as_ref()
            .map(|ob| {
                ob.items
                    .iter()
                    .map(|item| {
                        self.bind_expr(&item.expression).map(|be| BoundOrderByItem {
                            expression: be,
                            direction: item.direction,
                        })
                    })
                    .collect::<DBResult<Vec<_>>>()
            })
            .transpose()?;

        let delete_clause = stmt
            .delete_clause
            .as_ref()
            .map(|dc| self.bind_match_delete(dc))
            .transpose()?;

        Ok(BoundStatement::Match(BoundMatchStatement {
            query_graph,
            where_clause,
            return_clause,
            order_by,
            limit: stmt.limit.clone(),
            skip: stmt.skip.clone(),
            optional: stmt.optional,
            delete_clause,
        }))
    }

    pub(crate) fn bind_match_delete(
        &mut self,
        dc: &crate::parser::ast::MatchDeleteClause,
    ) -> DBResult<BoundMatchDeleteClause> {
        let target = match &dc.target {
            MatchDeleteTarget::Vertices(exprs) => {
                let bound = exprs
                    .iter()
                    .map(|e| self.bind_expr(e))
                    .collect::<DBResult<Vec<_>>>()?;
                BoundMatchDeleteTarget::Vertices(bound)
            }
            MatchDeleteTarget::Edges(exprs) => {
                let bound = exprs
                    .iter()
                    .map(|e| self.bind_expr(e))
                    .collect::<DBResult<Vec<_>>>()?;
                BoundMatchDeleteTarget::Edges(bound)
            }
            MatchDeleteTarget::EdgeRefs(refs) => {
                let mut bound = Vec::new();
                for (src, dst, rank) in refs {
                    let bsrc = self.bind_expr(src)?;
                    let bdst = self.bind_expr(dst)?;
                    let brank = rank.as_ref().map(|r| self.bind_expr(r)).transpose()?;
                    bound.push((bsrc, bdst, brank));
                }
                BoundMatchDeleteTarget::EdgeRefs(bound)
            }
        };
        Ok(BoundMatchDeleteClause {
            target,
            with_edge: dc.with_edge,
        })
    }

    /// Bind the body of an EXISTS / IN subquery into a nested MATCH
    /// statement.
    ///
    /// The subquery patterns (stored as re-parseable strings) are parsed
    /// into pattern ASTs and bound inside a child scope whose parent is the
    /// enclosing scope, so variables defined by the outer query resolve as
    /// correlated references.
    pub(crate) fn bind_subquery_body(
        &mut self,
        body: &graphdb_core::types::expr::SubqueryBody,
    ) -> DBResult<BoundStatement> {
        let parent = self.scope.clone();
        let outer_scope = std::mem::replace(&mut self.scope, BinderScope::with_parent(parent));
        let result = self.bind_subquery_body_inner(body);
        self.scope = outer_scope;
        result
    }

    fn bind_subquery_body_inner(
        &mut self,
        body: &graphdb_core::types::expr::SubqueryBody,
    ) -> DBResult<BoundStatement> {
        let mut patterns = Vec::with_capacity(body.patterns.len());
        let mut parser = crate::parser::parsing::TraversalParser::new();
        for pattern_str in &body.patterns {
            let pattern = parser
                .parse_pattern(&mut crate::parser::ParseContext::new(pattern_str))
                .map_err(|e| {
                    DBError::from(graphdb_core::error::QueryError::invalid_query(format!(
                        "Invalid subquery pattern `{pattern_str}`: {e}"
                    )))
                })?;
            patterns.push(pattern);
        }

        let query_graph = self.build_query_graph(&patterns)?;

        // Register subquery MATCH variables in the child scope before
        // binding the subquery WHERE / RETURN expressions.
        for node in &query_graph.nodes {
            self.scope.define_variable(BinderVariable {
                name: node.variable.clone(),
                alias_type: AliasType::Node,
                tags: node.tags.iter().map(|t| t.tag_name.to_string()).collect(),
                properties: node
                    .tags
                    .iter()
                    .flat_map(|t| t.properties.clone())
                    .collect(),
                is_defined: true,
            });
        }
        for edge in &query_graph.edges {
            self.scope.define_variable(BinderVariable {
                name: edge.variable.clone(),
                alias_type: AliasType::Edge,
                tags: edge
                    .edge_types
                    .iter()
                    .map(|e| e.edge_type_name.to_string())
                    .collect(),
                properties: edge
                    .edge_types
                    .iter()
                    .flat_map(|e| e.properties.clone())
                    .collect(),
                is_defined: true,
            });
        }

        let where_clause = body
            .where_clause
            .as_ref()
            .map(|cond| -> DBResult<BoundWhereClause> {
                let bound = self.bind_expr(&Self::plain_expression(cond.as_ref().clone()))?;
                Ok(BoundWhereClause { condition: bound })
            })
            .transpose()?;

        let return_clause = body
            .return_expr
            .as_ref()
            .map(|ret| -> DBResult<BoundReturnClause> {
                let bound = self.bind_expr(&Self::plain_expression(ret.as_ref().clone()))?;
                Ok(BoundReturnClause {
                    items: vec![BoundReturnItem {
                        expression: bound,
                        alias: None,
                    }],
                    distinct: false,
                    order_by: None,
                    limit: None,
                    skip: None,
                    sample: None,
                })
            })
            .transpose()?;

        Ok(BoundStatement::Match(BoundMatchStatement {
            query_graph,
            where_clause,
            return_clause,
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        }))
    }

    fn build_query_graph(&mut self, patterns: &[Pattern]) -> DBResult<QueryGraph> {
        let mut graph = QueryGraph::new();

        for pattern in patterns {
            self.process_pattern(pattern, &mut graph, None)?;
        }

        Ok(graph)
    }

    fn process_pattern(
        &mut self,
        pattern: &Pattern,
        graph: &mut QueryGraph,
        prev_node_var: Option<String>,
    ) -> DBResult<Option<String>> {
        match pattern {
            Pattern::Node(np) => {
                let var = np
                    .variable
                    .clone()
                    .unwrap_or_else(|| format!("__anon_n{}", graph.node_count()));

                let tags = self.resolve_tags(&np.labels)?;
                graph.add_node(BoundNodePattern {
                    variable: var.clone(),
                    tags,
                });
                Ok(Some(var))
            }
            Pattern::Edge(ep) => {
                let var = ep
                    .variable
                    .clone()
                    .unwrap_or_else(|| format!("__anon_e{}", graph.edge_count()));

                let edge_types = self.resolve_edge_types(&ep.edge_types)?;

                let src = prev_node_var
                    .clone()
                    .unwrap_or_else(|| format!("__src_{}", var));
                let dst = format!("__dst_{}", var);

                graph.add_edge(BoundEdgePattern {
                    variable: var,
                    edge_types,
                    direction: ep.direction,
                    src_variable: src,
                    dst_variable: dst,
                });
                Ok(None)
            }
            Pattern::Path(pp) => {
                let mut last_var = prev_node_var;
                for element in &pp.elements {
                    last_var = self.process_path_element(element, graph, last_var)?;
                }
                Ok(last_var)
            }
            Pattern::Variable(vp) => {
                if !self.scope.contains(&vp.name) {
                    return Err(DBError::from(
                        graphdb_core::error::QueryError::invalid_query(format!(
                            "Undefined variable: {}",
                            vp.name
                        )),
                    ));
                }
                Ok(Some(vp.name.clone()))
            }
        }
    }

    fn process_path_element(
        &mut self,
        element: &PathElement,
        graph: &mut QueryGraph,
        prev_node_var: Option<String>,
    ) -> DBResult<Option<String>> {
        match element {
            PathElement::Node(np) => {
                let var = np
                    .variable
                    .clone()
                    .unwrap_or_else(|| format!("__anon_n{}", graph.node_count()));
                let tags = self.resolve_tags(&np.labels)?;
                graph.add_node(BoundNodePattern {
                    variable: var.clone(),
                    tags,
                });
                Ok(Some(var))
            }
            PathElement::Edge(ep) => {
                let var = ep
                    .variable
                    .clone()
                    .unwrap_or_else(|| format!("__anon_e{}", graph.edge_count()));
                let edge_types = self.resolve_edge_types(&ep.edge_types)?;

                let src = prev_node_var
                    .clone()
                    .unwrap_or_else(|| format!("__src_{}", var));
                let dst = format!("__dst_{}", var);

                graph.add_edge(BoundEdgePattern {
                    variable: var,
                    edge_types,
                    direction: ep.direction,
                    src_variable: src,
                    dst_variable: dst,
                });
                Ok(None)
            }
            PathElement::Alternative(alt_patterns) => {
                for alt in alt_patterns {
                    self.process_pattern(alt, graph, prev_node_var.clone())?;
                }
                Ok(None)
            }
            PathElement::Optional(elem) => self.process_path_element(elem, graph, prev_node_var),
            PathElement::Repeated(elem, _rep) => {
                self.process_path_element(elem, graph, prev_node_var)
            }
        }
    }

    // ── Catalog resolution ─────────────────────────────────────────────────

    pub(crate) fn resolve_tags(&self, labels: &[String]) -> DBResult<Vec<BoundTagRef>> {
        if labels.is_empty() {
            return Ok(vec![]);
        }

        let mut resolved = Vec::new();
        if let Some(ref sm) = self.schema_manager {
            if let Some(ref space_name) = self.space_name {
                for label in labels {
                    let tag_info =
                        sm.get_tag(space_name, label)
                            .map_err(|e| {
                                DBError::from(graphdb_core::error::QueryError::invalid_query(
                                    format!("Failed to resolve tag '{}': {}", label, e),
                                ))
                            })?
                            .ok_or_else(|| {
                                DBError::from(graphdb_core::error::QueryError::invalid_query(
                                    format!("Tag '{}' not found in space '{}'", label, space_name),
                                ))
                            })?;

                    let mut properties = std::collections::HashMap::new();
                    for prop in &tag_info.properties {
                        properties.insert(
                            prop.name.clone(),
                            ValueType::from_data_type(&prop.data_type),
                        );
                    }

                    resolved.push(BoundTagRef {
                        tag_name: self.interner.intern(label),
                        properties,
                    });
                }
                return Ok(resolved);
            }
        }

        for label in labels {
            resolved.push(BoundTagRef {
                tag_name: self.interner.intern(label),
                properties: std::collections::HashMap::new(),
            });
        }
        Ok(resolved)
    }

    pub(crate) fn resolve_edge_types(
        &self,
        edge_types: &[String],
    ) -> DBResult<Vec<BoundEdgeTypeRef>> {
        if edge_types.is_empty() {
            return Ok(vec![]);
        }

        let mut resolved = Vec::new();
        if let Some(ref sm) = self.schema_manager {
            if let Some(ref space_name) = self.space_name {
                for et in edge_types {
                    let edge_info = sm
                        .get_edge_type(space_name, et)
                        .map_err(|e| {
                            DBError::from(graphdb_core::error::QueryError::invalid_query(format!(
                                "Failed to resolve edge type '{}': {}",
                                et, e
                            )))
                        })?
                        .ok_or_else(|| {
                            DBError::from(graphdb_core::error::QueryError::invalid_query(format!(
                                "Edge type '{}' not found in space '{}'",
                                et, space_name
                            )))
                        })?;

                    let mut properties = std::collections::HashMap::new();
                    for prop in &edge_info.properties {
                        properties.insert(
                            prop.name.clone(),
                            ValueType::from_data_type(&prop.data_type),
                        );
                    }

                    resolved.push(BoundEdgeTypeRef {
                        edge_type_name: self.interner.intern(et),
                        properties,
                    });
                }
                return Ok(resolved);
            }
        }

        for et in edge_types {
            resolved.push(BoundEdgeTypeRef {
                edge_type_name: self.interner.intern(et),
                properties: std::collections::HashMap::new(),
            });
        }
        Ok(resolved)
    }
}
