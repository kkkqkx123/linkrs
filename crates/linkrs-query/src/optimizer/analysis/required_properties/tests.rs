#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::access::graph_scan_node::{
        GetNeighborsNode, GetVerticesNode, ScanVerticesNode,
    };
    use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use crate::planning::plan::core::nodes::PlanNodeEnum;
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
    use linkrs_core::types::expr::ExpressionMeta;
    use linkrs_core::types::expr::FunctionArg;
    use linkrs_core::types::operators::BinaryOperator;
    use linkrs_core::{Value, YieldColumn};
    use std::sync::Arc;

    fn contextual(expr: Expression) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    fn prop(var: &str, name: &str) -> Expression {
        Expression::Property {
            object: Box::new(Expression::Variable(var.to_string())),
            property: name.to_string(),
        }
    }

    fn yield_column(expr: Expression, alias: &str) -> YieldColumn {
        YieldColumn {
            expression: contextual(expr),
            alias: alias.to_string(),
        }
    }

    fn get_vertices(var: &str) -> PlanNodeEnum {
        let mut node = GetVerticesNode::new(1, "test", var);
        node.set_output_var(var.to_string());
        PlanNodeEnum::GetVertices(node)
    }

    #[test]
    fn test_analyzer_collects_property_requirements() {
        // Project(v.age, v.name) <- GetVertices(v)
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                get_vertices("v"),
                vec![
                    yield_column(prop("v", "age"), "age"),
                    yield_column(prop("v", "name"), "name"),
                ],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let vid = match &project {
            PlanNodeEnum::Project(p) => p.input().id(),
            _ => unreachable!(),
        };
        let props = map
            .narrowable_properties(vid, "v")
            .expect("vertex must be narrowable");
        assert_eq!(props, vec!["age".to_string(), "name".to_string()]);
    }

    #[test]
    fn test_analyzer_collects_filter_requirements() {
        // Filter(v.age > 30) <- GetVertices(v)
        let filter = PlanNodeEnum::Filter(
            FilterNode::new(
                get_vertices("v"),
                contextual(Expression::Binary {
                    left: Box::new(prop("v", "age")),
                    op: BinaryOperator::GreaterThan,
                    right: Box::new(Expression::Literal(Value::Int(30))),
                }),
            )
            .expect("filter node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&filter);
        let PlanNodeEnum::Filter(f) = &filter else {
            unreachable!()
        };
        let props = map
            .narrowable_properties(f.input().id(), "v")
            .expect("vertex must be narrowable");
        assert_eq!(props, vec!["age".to_string()]);
    }

    #[test]
    fn test_analyzer_bare_variable_blocks_narrowing() {
        // Project(v AS v) <- GetVertices(v): the bare variable needs the
        // full vertex, so the binding must not be narrowable.
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                get_vertices("v"),
                vec![yield_column(Expression::Variable("v".to_string()), "v")],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let vid = match &project {
            PlanNodeEnum::Project(p) => p.input().id(),
            _ => unreachable!(),
        };
        assert!(map.narrowable_properties(vid, "v").is_none());
    }

    #[test]
    fn test_analyzer_function_argument_blocks_narrowing() {
        // Project(id(v) AS id) <- GetVertices(v): function arguments need
        // the full vertex, so the binding must not be narrowable.
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                get_vertices("v"),
                vec![yield_column(
                    Expression::Function {
                        name: "id".to_string(),
                        args: vec![FunctionArg::Positional(Expression::Variable(
                            "v".to_string(),
                        ))],
                    },
                    "id",
                )],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let vid = match &project {
            PlanNodeEnum::Project(p) => p.input().id(),
            _ => unreachable!(),
        };
        assert!(map.narrowable_properties(vid, "v").is_none());
    }

    #[test]
    fn test_analyzer_matches_binding_variable() {
        // Project(v.age, w.name) <- InnerJoin(GetVertices(v), GetVertices(w)):
        // each binding is narrowed to its own requirements only.
        let left = get_vertices("v");
        let right = get_vertices("w");
        let join = crate::planning::plan::core::nodes::join::InnerJoinNode::new(
            left,
            right,
            vec![],
            vec![],
        )
        .expect("join node");
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                PlanNodeEnum::InnerJoin(join),
                vec![
                    yield_column(prop("v", "age"), "age"),
                    yield_column(prop("w", "name"), "name"),
                ],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let PlanNodeEnum::Project(p) = &project else {
            unreachable!()
        };
        let PlanNodeEnum::InnerJoin(join) = p.input() else {
            unreachable!()
        };
        let props_v = map
            .narrowable_properties(join.left_input().id(), "v")
            .expect("left binding must be narrowable");
        let props_w = map
            .narrowable_properties(join.right_input().id(), "w")
            .expect("right binding must be narrowable");
        assert_eq!(props_v, vec!["age".to_string()]);
        assert_eq!(props_w, vec!["name".to_string()]);
    }

    #[test]
    fn test_analyzer_tag_resolution() {
        // A single-tag GetVertices resolves the tag on the requirement.
        let mut node = GetVerticesNode::new(1, "test", "v");
        node.set_output_var("v".to_string());
        node.set_tag_props(vec![crate::planning::plan::core::common::TagProp {
            tag: "person".to_string(),
            props: vec!["age".to_string()],
        }]);
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                PlanNodeEnum::GetVertices(node),
                vec![yield_column(prop("v", "age"), "age")],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let PlanNodeEnum::Project(p) = &project else {
            unreachable!()
        };
        let vid = p.input().id();
        let req = map
            .requirement_for_var(vid, "v")
            .expect("requirement must exist");
        assert_eq!(req.tag_name.as_deref(), Some("person"));
    }

    #[test]
    fn test_analyzer_get_neighbors_narrowing() {
        let mut node = GetNeighborsNode::new(1, "v");
        node.set_output_var("v".to_string());
        let neighbors = PlanNodeEnum::GetNeighbors(node);
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                neighbors.clone(),
                vec![yield_column(prop("v", "city"), "city")],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let PlanNodeEnum::Project(p) = &project else {
            unreachable!()
        };
        let props = map
            .narrowable_properties(p.input().id(), "v")
            .expect("neighbors must be narrowable");
        assert_eq!(props, vec!["city".to_string()]);
    }

    #[test]
    fn test_scan_vertices_uses_col_names_var() {
        let mut scan = ScanVerticesNode::new(0, "test");
        scan.set_col_names(vec!["n".to_string()]);
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                PlanNodeEnum::ScanVertices(scan),
                vec![yield_column(prop("n", "age"), "age")],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let PlanNodeEnum::Project(p) = &project else {
            unreachable!()
        };
        let props = map
            .narrowable_properties(p.input().id(), "n")
            .expect("scan must be narrowable");
        assert_eq!(props, vec!["age".to_string()]);
    }

    #[test]
    fn test_analyzer_tag_property_narrows_scan() {
        let mut scan = ScanVerticesNode::new(0, "test");
        scan.set_tag("person");
        scan.set_col_names(vec!["n".to_string()]);
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                PlanNodeEnum::ScanVertices(scan),
                vec![yield_column(
                    Expression::TagProperty {
                        tag_name: "person".to_string(),
                        property: "age".to_string(),
                    },
                    "age",
                )],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let PlanNodeEnum::Project(pr) = &project else {
            unreachable!()
        };
        let props = map
            .narrowable_properties(pr.input().id(), "n")
            .expect("tag property must narrow");
        assert_eq!(props, vec!["age".to_string()]);
    }

    #[test]
    fn test_analyzer_struct_field_narrows_top_level() {
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                get_vertices("v"),
                vec![yield_column(
                    Expression::StructField {
                        base: Box::new(Expression::Variable("v".to_string())),
                        field: "addr".to_string(),
                    },
                    "addr",
                )],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let PlanNodeEnum::Project(pr) = &project else {
            unreachable!()
        };
        let props = map
            .narrowable_properties(pr.input().id(), "v")
            .expect("struct field must narrow");
        assert_eq!(props, vec!["addr".to_string()]);
    }

    #[test]
    fn test_analyzer_function_arg_property_narrows() {
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                get_vertices("v"),
                vec![yield_column(
                    Expression::function("lower", vec![prop("v", "name")]),
                    "name",
                )],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let PlanNodeEnum::Project(pr) = &project else {
            unreachable!()
        };
        let props = map
            .narrowable_properties(pr.input().id(), "v")
            .expect("function arg must narrow");
        assert_eq!(props, vec!["name".to_string()]);
    }
}
