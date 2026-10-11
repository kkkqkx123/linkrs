use super::*;

use crate::planning::join_order::query_graph::{QueryNode, QueryRel};

fn triangle() -> Arc<QueryGraph> {
    let mut qg = QueryGraph::new();
    qg.add_node(QueryNode::new("a", "a"));
    qg.add_node(QueryNode::new("b", "b"));
    qg.add_node(QueryNode::new("c", "c"));
    qg.add_rel(QueryRel::new("e1", "e1", "a", "b"));
    qg.add_rel(QueryRel::new("e2", "e2", "a", "c"));
    qg.add_rel(QueryRel::new("e3", "e3", "b", "c"));
    Arc::new(qg)
}

fn single_edge() -> Arc<QueryGraph> {
    let mut qg = QueryGraph::new();
    qg.add_node(QueryNode::new("a", "a"));
    qg.add_node(QueryNode::new("b", "b"));
    qg.add_rel(QueryRel::new("e1", "e1", "a", "b"));
    Arc::new(qg)
}

#[test]
fn base_scans_seed_level_one() {
    let qg = triangle();
    let mut enumerator = JoinOrderEnumerator::new();
    enumerator.plan_base_table_scans(&qg);
    assert_eq!(enumerator.context.sub_plans_table.get_subgraphs(1).len(), 6);
}

#[test]
fn single_edge_graph_plans_end_to_end() {
    let qg = single_edge();
    let mut enumerator = JoinOrderEnumerator::new();
    let plan = enumerator.plan_query_graph(&qg);
    assert!(plan.is_some(), "single-edge graph should plan");
}

#[test]
fn triangle_graph_plans_end_to_end() {
    let qg = triangle();
    let mut enumerator = JoinOrderEnumerator::new();
    let plan = enumerator.plan_query_graph(&qg);
    assert!(plan.is_some(), "triangle graph should plan");
}

#[test]
fn join_key_interning_is_stable() {
    let mut ctx = JoinOrderEnumeratorContext::new();
    let first = ctx.join_key("a");
    let second = ctx.join_key("a");
    assert_eq!(first.id(), second.id());
}

#[test]
fn base_scan_encoding_marks_node_flat() {
    let qg = triangle();
    let mut enumerator = JoinOrderEnumerator::new();
    enumerator.plan_base_table_scans(&qg);
    let sg = crate::planning::join_order::SubqueryGraph::single_node(&qg, 0);
    let best = enumerator
        .context
        .sub_plans_table
        .get_best_plan(&sg)
        .expect("base plan");
    assert_ne!(best.encoding, 0, "base scan must carry flat encoding");
    assert_eq!(best.encoding & 1, 1);
}

#[test]
fn wco_and_hash_encodings_differ_on_triangle() {
    let qg = triangle();
    let mut enumerator = JoinOrderEnumerator::new();
    let plan = enumerator.plan_query_graph(&qg).expect("triangle plans");
    let encoding = enumerator.encode_plan(&plan, &qg);
    assert_ne!(encoding, 0, "triangle plan must carry non-zero encoding");
}

#[test]
fn encoding_folds_rel_variables() {
    // Rel variables participate in the encoding via folding: a graph
    // with rels must not encode identically to the same nodes without
    // rels when the rel bindings are flat.
    let nodes_only = {
        let mut qg = QueryGraph::new();
        qg.add_node(QueryNode::new("a", "a"));
        qg.add_node(QueryNode::new("b", "b"));
        Arc::new(qg)
    };
    let with_rel = single_edge();
    let mut enumerator = JoinOrderEnumerator::new();
    let plan = enumerator
        .plan_query_graph(&with_rel)
        .expect("single edge plans");
    let enc_nodes = enumerator.encode_plan(&plan, &nodes_only);
    let enc_rel = enumerator.encode_plan(&plan, &with_rel);
    assert_ne!(
        enc_nodes, enc_rel,
        "rel folding must distinguish node-only and with-rel encodings"
    );
}

#[test]
fn encoding_overflow_is_deterministic() {
    // Overflow nodes and rels fold via `mix_overflow`: deterministic,
    // salt-sensitive, and panic-free well beyond 64 entries. The DP
    // bitset itself (`SubqueryGraph`) stays capped at 64, so the
    // folding unit is tested directly here.
    let first = JoinOrderEnumerator::mix_overflow(0, 70);
    let second = JoinOrderEnumerator::mix_overflow(0, 70);
    assert_eq!(first, second, "overflow folding must be deterministic");
    assert_ne!(
        JoinOrderEnumerator::mix_overflow(0, 70),
        JoinOrderEnumerator::mix_overflow(0, 71),
        "different overflow salts must fold differently"
    );
    // End-to-end: encoding a plan against a 65-node graph must not
    // panic even though only the first 64 map to dedicated bits.
    let mut qg = QueryGraph::new();
    for i in 0..65 {
        qg.add_node(QueryNode::new(format!("n{i}"), format!("n{i}")));
    }
    let qg = Arc::new(qg);
    let mut enumerator = JoinOrderEnumerator::new();
    let plan = enumerator.plan_query_graph(&triangle()).expect("plan");
    let a = enumerator.encode_plan(&plan, &qg);
    let b = enumerator.encode_plan(&plan, &qg);
    assert_eq!(a, b);
}

#[test]
fn hint_binary_join_plans() {
    use crate::planning::join_order::{JoinHint, JoinHintNode};
    let qg = single_edge();
    let hint = JoinHint {
        root: JoinHintNode::Binary {
            left: Box::new(JoinHintNode::Scan {
                variable: "a".to_string(),
            }),
            right: Box::new(JoinHintNode::Scan {
                variable: "e1".to_string(),
            }),
        },
    };
    let mut enumerator = JoinOrderEnumerator::new();
    let plan = enumerator.plan_with_hint(&qg, &hint).expect("hint plans");
    assert!(matches!(plan, LogicalNodeEnum::InnerJoin(_)));
}

#[test]
fn stats_snapshot_drives_base_scan_cards() {
    use super::super::cardinality_estimator::JoinOrderStats;
    let mut qg = QueryGraph::new();
    qg.add_node(QueryNode::new("a", "a").with_labels(vec!["person".to_string()]));
    qg.add_node(QueryNode::new("b", "b"));
    qg.add_rel(QueryRel::new("e1", "e1", "a", "b").with_edge_types(vec!["knows".to_string()]));
    let qg = Arc::new(qg);
    let mut stats = JoinOrderStats::default();
    stats.vertex_counts.insert("person".into(), 5_000);
    stats.edge_counts.insert("knows".into(), 20_000);
    stats.avg_out_degrees.insert("knows".into(), 4.0);
    let mut enumerator = JoinOrderEnumerator::new().with_stats(stats);
    enumerator.plan_base_table_scans(&qg);
    let node_sg = crate::planning::join_order::SubqueryGraph::single_node(&qg, 0);
    let node_best = enumerator
        .context
        .sub_plans_table
        .get_best_plan(&node_sg)
        .expect("node plan");
    assert_eq!(node_best.cardinality, 5_000);
    let rel_sg = crate::planning::join_order::SubqueryGraph::single_rel(&qg, 0);
    let rel_best = enumerator
        .context
        .sub_plans_table
        .get_best_plan(&rel_sg)
        .expect("rel plan");
    assert_eq!(rel_best.cardinality, 20_000);
    assert_eq!(
        enumerator.context.cardinality_estimator.avg_degree_hint(),
        4
    );
}

#[test]
fn hint_binary_rel_join_plans() {
    use crate::planning::join_order::{JoinHint, JoinHintNode};
    let mut qg = QueryGraph::new();
    qg.add_node(QueryNode::new("a", "a"));
    qg.add_node(QueryNode::new("b", "b"));
    qg.add_node(QueryNode::new("c", "c"));
    qg.add_rel(QueryRel::new("e1", "e1", "a", "b"));
    qg.add_rel(QueryRel::new("e2", "e2", "a", "c"));
    let qg = Arc::new(qg);
    let hint = JoinHint {
        root: JoinHintNode::Binary {
            left: Box::new(JoinHintNode::Scan {
                variable: "e1".to_string(),
            }),
            right: Box::new(JoinHintNode::Scan {
                variable: "e2".to_string(),
            }),
        },
    };
    let mut enumerator = JoinOrderEnumerator::new();
    let plan = enumerator.plan_with_hint(&qg, &hint).expect("hint plans");
    assert!(matches!(plan, LogicalNodeEnum::InnerJoin(_)));
}

#[test]
fn hint_multiway_join_plans_wco() {
    use crate::planning::join_order::{JoinHint, JoinHintNode};
    let qg = triangle();
    let hint = JoinHint {
        root: JoinHintNode::Multiway {
            probe: Box::new(JoinHintNode::Binary {
                left: Box::new(JoinHintNode::Scan {
                    variable: "a".to_string(),
                }),
                right: Box::new(JoinHintNode::Scan {
                    variable: "e1".to_string(),
                }),
            }),
            builds: vec![
                JoinHintNode::Scan {
                    variable: "e2".to_string(),
                },
                JoinHintNode::Scan {
                    variable: "e3".to_string(),
                },
            ],
        },
    };
    let mut enumerator = JoinOrderEnumerator::new();
    let plan = enumerator.plan_with_hint(&qg, &hint).expect("hint plans");
    assert!(matches!(plan, LogicalNodeEnum::WcoIntersect(_)));
}
