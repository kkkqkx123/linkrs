use std::collections::HashMap;

use graphdb_core::types::expr::ExpressionId;

use crate::executor::streaming::factorized_table::{ColumnSchema, FactorizedTableSchema};
#[cfg(test)]
use crate::planning::plan::factorization::FGroupPos;
use crate::planning::plan::factorization::FactorizedSchema;

/// Row layout size helper mirroring `LogicalTypeUtils::getRowLayoutSize`.
fn row_layout_size_for_expr(_expr_id: &ExpressionId) -> u32 {
    // Simplified: assume 16 bytes for most types; overflow pointers are also 16.
    16
}

/// Create a FactorizedTableSchema from a list of expressions and their
/// FactorizedSchema groups.
///
/// Mirrors `FactorizedTableUtils::createFTableSchema` in
/// `ref/ladybug/src/processor/result/factorized_table_util.cpp`.
pub fn create_ftable_schema(
    expressions: &[ExpressionId],
    schema: &FactorizedSchema,
) -> FactorizedTableSchema {
    let mut columns = Vec::with_capacity(expressions.len());
    for expr_id in expressions {
        let group_pos = schema.get_group_pos(expr_id).unwrap_or_else(|| {
            panic!(
                "create_ftable_schema: expression {:?} not in FactorizedSchema; silent fallback to group 0 would corrupt flat/unflat layout",
                expr_id
            )
        });
        let group = schema
            .get_group(group_pos)
            .expect("create_ftable_schema: group_pos from schema must be valid");
        let is_flat = group.is_flat();
        if is_flat {
            columns.push(ColumnSchema {
                is_unflat: false,
                group_id: group_pos,
                num_bytes: row_layout_size_for_expr(expr_id),
                may_contain_nulls: true,
            });
        } else {
            columns.push(ColumnSchema {
                is_unflat: true,
                group_id: group_pos,
                num_bytes: std::mem::size_of::<
                    crate::executor::streaming::factorized_table::OverflowValue,
                >() as u32,
                may_contain_nulls: true,
            });
        }
    }
    FactorizedTableSchema::new(columns)
}

/// Extended version that resolves types via an optional type map.
pub fn create_ftable_schema_with_types(
    expressions: &[ExpressionId],
    schema: &FactorizedSchema,
    type_map: &HashMap<ExpressionId, String>,
) -> FactorizedTableSchema {
    let mut columns = Vec::with_capacity(expressions.len());
    for expr_id in expressions {
        let group_pos = schema.get_group_pos(expr_id).unwrap_or_else(|| {
            panic!(
                "create_ftable_schema_with_types: expression {:?} not in FactorizedSchema",
                expr_id
            )
        });
        let group = schema
            .get_group(group_pos)
            .expect("create_ftable_schema_with_types: group_pos must be valid");
        let is_flat = group.is_flat();
        let typ = type_map
            .get(expr_id)
            .map(|s| s.as_str())
            .unwrap_or("unknown");
        let num_bytes = if is_flat {
            match typ.to_lowercase().as_str() {
                "int" | "bigint" => 8,
                "double" | "float" => 8,
                "bool" => 1,
                "string" => 16,
                _ => 16,
            }
        } else {
            std::mem::size_of::<crate::executor::streaming::factorized_table::OverflowValue>()
                as u32
        };
        columns.push(ColumnSchema {
            is_unflat: !is_flat,
            group_id: group_pos,
            num_bytes,
            may_contain_nulls: true,
        });
    }
    FactorizedTableSchema::new(columns)
}

#[cfg(test)]
pub fn create_ftable_schema_from_groups(
    groups: &[(String, FGroupPos, bool)],
) -> FactorizedTableSchema {
    let mut columns = Vec::with_capacity(groups.len());
    for (_name, gid, is_flat) in groups {
        columns.push(ColumnSchema {
            is_unflat: !*is_flat,
            group_id: *gid,
            num_bytes: if *is_flat {
                16
            } else {
                std::mem::size_of::<crate::executor::streaming::factorized_table::OverflowValue>()
                    as u32
            },
            may_contain_nulls: true,
        });
    }
    FactorizedTableSchema::new(columns)
}

#[cfg(test)]
pub fn create_ftable_schema_for_logical(
    node: &mut crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum,
    child_schemas: &[FactorizedSchema],
) -> FactorizedTableSchema {
    use crate::planning::plan::factorization::FactorizedSchemaCompute;
    let schema = node.compute_factorized_schema(child_schemas);
    if node.col_names().is_empty() {
        return FactorizedTableSchema::new(Vec::new());
    }
    let mut columns = Vec::with_capacity(node.col_names().len());
    for name in node.col_names() {
        let pos = schema.get_group_pos_by_name(name).unwrap_or_else(|| {
            panic!(
                "create_ftable_schema_for_logical (test-only): missing group for column '{}' not in FactorizedSchema",
                name
            )
        });
        let group = schema
            .get_group(pos)
            .expect("create_ftable_schema_for_logical: group must exist");
        let is_flat = group.is_flat();
        columns.push(ColumnSchema {
            is_unflat: !is_flat,
            group_id: pos,
            num_bytes: if is_flat {
                16
            } else {
                std::mem::size_of::<crate::executor::streaming::factorized_table::OverflowValue>()
                    as u32
            },
            may_contain_nulls: true,
        });
    }
    FactorizedTableSchema::new(columns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::factorization::FactorizedSchema;
    use graphdb_core::types::expr::ExpressionId;

    fn eid(id: u64) -> ExpressionId {
        ExpressionId::new(id)
    }

    #[test]
    fn bridge_flat_and_unflat() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        let a = eid(1);
        let b = eid(2);
        schema.insert_to_group_and_scope(a.clone(), g0);
        schema.insert_to_group_and_scope(b.clone(), g1);

        let ftable_schema = create_ftable_schema(&[a.clone(), b.clone()], &schema);
        assert_eq!(ftable_schema.num_columns(), 2);
        assert_eq!(ftable_schema.get_column(0).expect("col0").is_flat(), true);
        assert_eq!(ftable_schema.get_column(0).expect("col0").group_id, g0);
        assert_eq!(ftable_schema.get_column(1).expect("col1").is_flat(), false);
        assert_eq!(ftable_schema.get_column(1).expect("col1").group_id, g1);
    }

    #[test]
    fn bridge_with_types() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        let a = eid(10);
        let b = eid(20);
        schema.insert_to_group_and_scope(a.clone(), g0);
        schema.insert_to_group_and_scope(b.clone(), g1);
        let mut type_map = HashMap::new();
        type_map.insert(a.clone(), "int".to_string());
        type_map.insert(b.clone(), "string".to_string());

        let fts = create_ftable_schema_with_types(&[a, b], &schema, &type_map);
        assert_eq!(fts.num_columns(), 2);
        assert_eq!(fts.get_column(0).expect("c0").num_bytes, 8);
        // unflat column uses overflow size
        assert!(fts.get_column(1).expect("c1").is_unflat);
    }

    #[test]
    fn dataflow_example_scan_extend_projection() {
        // Simulate docs dataflow example: MATCH (a)-[:Knows]->(b) RETURN a.name, b.name
        // Scan(a): group0 flat {a.ID, a.name}
        // Extend: group0 flat, group1 unflat {b.ID, b.name}
        // Projection: groups preserved.
        let mut scan_schema = FactorizedSchema::new();
        let g0 = scan_schema.create_flat_group(false);
        let a_id = eid(100);
        let a_name = eid(101);
        scan_schema.insert_to_group_and_scope(a_id.clone(), g0);
        scan_schema.insert_to_group_and_scope(a_name.clone(), g0);

        let mut extend_schema = scan_schema.copy();
        let g1 = extend_schema.create_group();
        let b_id = eid(200);
        let b_name = eid(201);
        extend_schema.insert_to_group_and_scope(b_id.clone(), g1);
        extend_schema.insert_to_group_and_scope(b_name.clone(), g1);
        assert_eq!(extend_schema.num_groups(), 2);
        assert!(extend_schema.get_group(g0).expect("g0").is_flat());
        assert!(!extend_schema.get_group(g1).expect("g1").is_flat());

        // Projection of a.name (g0) and b.name (g1)
        let fts = create_ftable_schema(&[a_name.clone(), b_name.clone()], &extend_schema);
        // Expect 2 columns: flat for a.name, unflat for b.name
        assert!(fts.get_column(0).expect("col0").is_flat());
        assert!(fts.get_column(1).expect("col1").is_unflat);
        // Build a FactorizedTable and verify flat tuple counts.
        let mut table = crate::executor::streaming::factorized_table::FactorizedTable::new(fts);
        // Row0: a=Alice, b=[Bob, Carl]
        table
            .append(&[
                vec![graphdb_core::Value::string("Alice")],
                vec![
                    graphdb_core::Value::string("Bob"),
                    graphdb_core::Value::string("Carl"),
                ],
            ])
            .unwrap();
        assert_eq!(table.num_flat_tuples_for_row(0), 2);
    }
}
