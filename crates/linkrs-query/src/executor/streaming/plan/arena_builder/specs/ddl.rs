//! Sink (write), DDL manage, fulltext and vector spec builders.

mod fulltext;
mod schema;
mod sink;
mod user;
mod vector;

pub use fulltext::*;
pub use schema::*;
pub use sink::*;
pub use user::*;
pub use vector::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::base::ExecutionContext;
    use crate::parser::ast::fulltext::{BM25Options, IndexFieldDef, IndexOptions};
    use crate::planning::plan::core::nodes::management::manage_node_enums::{
        FulltextManageNode, SequenceManageNode,
    };
    use crate::planning::plan::core::nodes::management::sequence_nodes::{
        AlterSequenceNode, CreateSequenceNode, DropSequenceNode, SequenceManageInfo,
    };
    use crate::planning::plan::core::nodes::search::fulltext::management::CreateFulltextIndexNode;
    use linkrs_core::types::FulltextEngineType;
    use std::collections::HashMap;

    fn exec_ctx() -> ExecutionContext {
        ExecutionContext::default()
    }

    #[test]
    fn sequence_create_spec_carries_all_parameters() {
        let info = SequenceManageInfo::new("s".to_string())
            .with_start(Some(1))
            .with_increment(Some(2))
            .with_bounds(Some(1), Some(100))
            .with_cycle(Some(true))
            .with_if_not_exists(true);
        let node = SequenceManageNode::Create(CreateSequenceNode::new(1, info));
        match build_sequence_manage_spec(&node, &exec_ctx()).expect("spec builds") {
            DdlSpec::SequenceManage {
                command:
                    crate::executor::streaming::operators::spec::SequenceManageCommand::Create {
                        seq_name,
                        start,
                        increment,
                        min_value,
                        max_value,
                        cycle,
                        if_not_exists,
                    },
            } => {
                assert_eq!(seq_name, "s");
                assert_eq!(start, Some(1));
                assert_eq!(increment, Some(2));
                assert_eq!(min_value, Some(1));
                assert_eq!(max_value, Some(100));
                assert!(cycle);
                assert!(if_not_exists);
            }
            other => panic!("expected SequenceManage::Create, got {other:?}"),
        }
    }

    #[test]
    fn sequence_alter_and_drop_specs_carry_flags() {
        let info = SequenceManageInfo::new("s".to_string())
            .with_increment(Some(5))
            .with_cycle(Some(false));
        let node = SequenceManageNode::Alter(AlterSequenceNode::new(2, info));
        match build_sequence_manage_spec(&node, &exec_ctx()).expect("spec builds") {
            DdlSpec::SequenceManage {
                command:
                    crate::executor::streaming::operators::spec::SequenceManageCommand::Alter {
                        seq_name,
                        increment,
                        cycle,
                        ..
                    },
            } => {
                assert_eq!(seq_name, "s");
                assert_eq!(increment, Some(5));
                assert_eq!(cycle, Some(false));
            }
            other => panic!("expected SequenceManage::Alter, got {other:?}"),
        }

        let node = SequenceManageNode::Drop(DropSequenceNode::new(3, "s".to_string()));
        match build_sequence_manage_spec(&node, &exec_ctx()).expect("spec builds") {
            DdlSpec::SequenceManage {
                command:
                    crate::executor::streaming::operators::spec::SequenceManageCommand::Drop {
                        seq_name,
                        if_exists,
                    },
            } => {
                assert_eq!(seq_name, "s");
                assert!(!if_exists);
            }
            other => panic!("expected SequenceManage::Drop, got {other:?}"),
        }
    }

    #[test]
    fn fulltext_create_spec_keeps_engine_fields_options_and_flag() {
        let fields = vec![IndexFieldDef {
            field_name: "content".to_string(),
            analyzer: None,
            boost: None,
        }];
        let options = IndexOptions {
            bm25_config: Some(BM25Options {
                k1: Some(1.2),
                b: Some(0.8),
                field_weights: HashMap::new(),
                analyzer: None,
                store_original: None,
            }),
            common_options: HashMap::new(),
        };
        let plan_node = CreateFulltextIndexNode::new(
            "idx_a".to_string(),
            "Article".to_string(),
            fields,
            FulltextEngineType::Bm25,
            options,
            true,
            7,
        );
        let node = FulltextManageNode::Create(plan_node);
        match fulltext_manage_to_command(&node) {
            FulltextManageCommand::Create {
                index_name,
                schema_name,
                fields,
                engine_type,
                options,
                if_not_exists,
                space_id,
            } => {
                assert_eq!(index_name, "idx_a");
                assert_eq!(schema_name, "Article");
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].field_name, "content");
                assert_eq!(engine_type, FulltextEngineType::Bm25);
                let bm25 = options.bm25_config.expect("bm25 options carried");
                assert_eq!(bm25.k1, Some(1.2));
                assert_eq!(bm25.b, Some(0.8));
                assert!(if_not_exists);
                assert_eq!(space_id, 7);
            }
            other => panic!("expected FulltextManageCommand::Create, got {other:?}"),
        }
    }

    #[test]
    fn fulltext_field_validation_accepts_matching_fields() {
        use crate::parser::ast::fulltext::FulltextQueryExpr;
        assert!(validate_fulltext_query_fields(
            &FulltextQueryExpr::Simple("hello".to_string()),
            "content"
        )
        .is_ok());
        assert!(validate_fulltext_query_fields(
            &FulltextQueryExpr::Field("content".to_string(), "hello".to_string()),
            "content"
        )
        .is_ok());
        assert!(validate_fulltext_query_fields(
            &FulltextQueryExpr::Boolean {
                must: vec![FulltextQueryExpr::Simple("a".to_string())],
                should: vec![],
                must_not: vec![],
            },
            "content"
        )
        .is_ok());
    }

    #[test]
    fn fulltext_field_validation_rejects_wrong_field() {
        use crate::parser::ast::fulltext::FulltextQueryExpr;
        match validate_fulltext_query_fields(
            &FulltextQueryExpr::Field("title".to_string(), "hello".to_string()),
            "content",
        ) {
            Err(PlanBuildError::CapabilityUnavailable { capability, .. }) => {
                assert_eq!(capability, "fulltext-field-mismatch");
            }
            other => panic!("expected capability error, got {other:?}"),
        }
    }
}
