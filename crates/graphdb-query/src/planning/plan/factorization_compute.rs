use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use graphdb_core::types::expr::ExpressionId;

use crate::planning::plan::factorization::{
    FGroupPos, FactorizedSchema, FactorizedSchemaCompute,
};
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

fn expr_id_for_name(name: &str) -> ExpressionId {
    let mut h = DefaultHasher::new();
    name.hash(&mut h);
    ExpressionId::new(h.finish())
}

impl FactorizedSchemaCompute for LogicalNodeEnum {
    fn compute_factorized_schema(
        &mut self,
        child_schemas: &[FactorizedSchema],
    ) -> FactorizedSchema {
        match self {
            LogicalNodeEnum::ScanVertices(n) => {
                let mut schema = FactorizedSchema::new();
                let g = schema.create_flat_group(false);
                for name in n.col_names() {
                    let eid = expr_id_for_name(name);
                    schema.insert_to_group_and_scope(eid, g);
                }
                if schema.num_groups() == 0 {
                    schema.create_flat_group(false);
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::ScanEdges(n) => {
                let mut schema = FactorizedSchema::new();
                let g = schema.create_flat_group(false);
                for name in n.col_names() {
                    let eid = expr_id_for_name(name);
                    schema.insert_to_group_and_scope(eid, g);
                }
                if schema.num_groups() == 0 {
                    schema.create_flat_group(false);
                }
                schema
            }
            LogicalNodeEnum::GetVertices(n) => {
                let mut schema = if let Some(cs) = child_schemas.first() {
                    cs.clone()
                } else {
                    FactorizedSchema::new()
                };
                if schema.num_groups() == 0 {
                    let g = schema.create_flat_group(false);
                    for name in n.col_names() {
                        schema.insert_to_group_and_scope(expr_id_for_name(name), g);
                    }
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::GetNeighbors(n) => {
                let mut schema = if let Some(cs) = child_schemas.first() {
                    cs.clone()
                } else {
                    FactorizedSchema::new()
                };
                if schema.num_groups() == 0 {
                    let g = schema.create_flat_group(false);
                    schema.insert_to_group_and_scope(expr_id_for_name("src"), g);
                }
                let g1 = schema.create_group();
                for name in n.col_names() {
                    if !schema.is_name_in_scope(name) {
                        schema.insert_to_group_and_scope(expr_id_for_name(name), g1);
                    }
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Flatten(n) => {
                let mut schema = if let Some(cs) = child_schemas.first() {
                    cs.clone()
                } else {
                    FactorizedSchema::new()
                };
                if (n.group_pos as usize) < schema.num_groups() {
                    schema.flatten_group(n.group_pos);
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Project(n) => {
                let schema = child_schemas.first().cloned().unwrap_or_default();
                if schema.num_groups() == 0 {
                    let mut out = FactorizedSchema::new();
                    let g = out.create_flat_group(false);
                    for col in &n.columns {
                        let name = &col.alias;
                        out.insert_to_group_and_scope(expr_id_for_name(name), g);
                    }
                    out.validate_at_most_one_unflat();
                    return out;
                }
                let mut out = schema.clone();
                out.validate_at_most_one_unflat();
                out
            }
            LogicalNodeEnum::Filter(_) => {
                let schema = child_schemas.first().cloned().unwrap_or_default();
                let mut out = schema.clone();
                out.validate_at_most_one_unflat();
                out
            }
            LogicalNodeEnum::Aggregate(n) => {
                let child = child_schemas.first().cloned().unwrap_or_default();
                let mut out = FactorizedSchema::new();
                let g = out.create_flat_group(false);
                for key in &n.group_keys {
                    out.insert_to_group_and_scope(expr_id_for_name(key), g);
                }
                for func in &n.aggregation_functions {
                    let name = format!("{:?}", func);
                    out.insert_to_group_and_scope(expr_id_for_name(&name), g);
                }
                if out.num_groups() == 0 && child.num_groups() > 0 {
                    let g2 = out.create_flat_group(false);
                    for eid in child.expressions_in_scope() {
                        out.insert_to_group_and_scope(eid.clone(), g2);
                    }
                }
                out.validate_at_most_one_unflat();
                out
            }
            LogicalNodeEnum::Sort(_)
            | LogicalNodeEnum::Limit(_)
            | LogicalNodeEnum::TopN(_)
            | LogicalNodeEnum::Sample(_)
            | LogicalNodeEnum::Dedup(_)
            | LogicalNodeEnum::Window(_) => {
                child_schemas.first().cloned().unwrap_or_default()
            }
            LogicalNodeEnum::InnerJoin(_)
            | LogicalNodeEnum::LeftJoin(_)
            | LogicalNodeEnum::RightJoin(_)
            | LogicalNodeEnum::CrossJoin(_)
            | LogicalNodeEnum::FullOuterJoin(_)
            | LogicalNodeEnum::SemiJoin(_) => {
                if child_schemas.len() >= 2 {
                    let left = &child_schemas[0];
                    let right = &child_schemas[1];
                    let mut merged = left.clone();
                    let mapping = merged.merge_groups_from(right);
                    for (expr_id, gpos) in right.expression_to_group_iter() {
                        let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
                        merged.insert_to_scope_may_repeat(expr_id.clone(), new_pos);
                    }
                    if merged.has_unflat_group() {
                        let unflat_count =
                            merged.groups().iter().filter(|g| !g.is_flat()).count();
                        if unflat_count > 1 {
                            let mut first = true;
                            for i in 0..merged.num_groups() {
                                let pos = i as FGroupPos;
                                if let Some(g) = merged.get_group(pos) {
                                    if !g.is_flat() {
                                        if first {
                                            first = false;
                                        } else {
                                            merged.flatten_group(pos);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    merged.validate_at_most_one_unflat();
                    merged
                } else {
                    child_schemas.first().cloned().unwrap_or_default()
                }
            }
            LogicalNodeEnum::Traverse(_)
            | LogicalNodeEnum::Expand(_)
            | LogicalNodeEnum::ExpandAll(_)
            | LogicalNodeEnum::AppendVertices(_)
            | LogicalNodeEnum::BiExpand(_)
            | LogicalNodeEnum::BiTraverse(_) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                if schema.has_unflat_group() {
                    if let Some(pos) = schema.unflat_group_pos() {
                        schema.flatten_group(pos);
                    }
                }
                let g = schema.create_group();
                let new_name = format!("traverse_{}", g);
                schema.insert_to_group_and_scope(expr_id_for_name(&new_name), g);
                schema.validate_at_most_one_unflat();
                schema
            }
            _ => child_schemas.first().cloned().unwrap_or_default(),
        }
    }

    fn compute_flat_schema(
        &mut self,
        child_schemas: &[FactorizedSchema],
    ) -> FactorizedSchema {
        let flat_children: Vec<FactorizedSchema> =
            child_schemas.iter().map(|cs| cs.flat_copy()).collect();
        let mut result = self.compute_factorized_schema(&flat_children);
        result.flatten_all();
        result.validate_at_most_one_unflat();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::node_id_generator::next_node_id;
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
    use crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;

    fn scan() -> LogicalNodeEnum {
        LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id: next_node_id(),
            space_id: 1,
            space_name: "test".to_string(),
            tag: Some("p".to_string()),
            expression: None,
            limit: None,
            projected_properties: vec![],
            output_var: None,
            col_names: vec!["a.name".to_string()],
            column_types: vec![],
        })
    }

    #[test]
    fn scan_schema_is_flat() {
        let mut n = scan();
        let s = n.compute_factorized_schema(&[]);
        assert!(s.is_flat_schema());
        assert_eq!(s.num_groups(), 1);
        let flat = n.compute_flat_schema(&[]);
        assert!(flat.is_flat_schema());
    }

    #[test]
    fn flatten_schema() {
        let mut scan_n = scan();
        let scan_schema = scan_n.compute_factorized_schema(&[]);
        let mut g = scan_schema.clone();
        let pos = g.create_group();
        g.insert_to_group_and_scope(expr_id_for_name("b.name"), pos);
        assert!(!g.get_group(pos).expect("g").is_flat());
        let mut flatten = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(pos, scan()));
        let out = flatten.compute_factorized_schema(&[g]);
        assert!(out.is_flat_schema());
    }

    #[test]
    fn join_merges() {
        let mut left = scan();
        let ls = left.compute_factorized_schema(&[]);
        let mut right = scan();
        let mut rs = right.compute_factorized_schema(&[]);
        let pos = rs.create_group();
        rs.insert_to_group_and_scope(expr_id_for_name("b"), pos);
        let mut join = LogicalNodeEnum::InnerJoin(
            crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode {
                id: next_node_id(),
                left: Box::new(scan()),
                right: Box::new(scan()),
                hash_keys: vec![],
                probe_keys: vec![],
                deps: vec![scan(), scan()],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        let out = join.compute_factorized_schema(&[ls, rs]);
        out.validate_at_most_one_unflat();
    }
}
