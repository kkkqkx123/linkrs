use super::{set_typed_columns_enabled, DataChunk, RowPool, TypedColumn, TypedKind};
    use crate::core::types::expr::Expression;
    use crate::core::types::operators::BinaryOperator;
    use crate::core::types::storage_ids::VertexId;
    use crate::core::Value;
    use crate::core::Vertex;
    use crate::query::executor::streaming::slot::SlotLayout;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    #[test]
    fn test_data_chunk_creation() {
        let rows = vec![vec![Value::string("a"), Value::string("b")]];
        let chunk = DataChunk::from_rows(rows);
        assert_eq!(chunk.len(), 1);
        assert_eq!(chunk.num_columns(), 2);
    }

    #[test]
    fn test_data_chunk_empty() {
        let chunk = DataChunk::from_rows(vec![]);
        assert!(chunk.is_empty());
        assert_eq!(chunk.num_columns(), 0);
    }

    #[test]
    fn test_data_chunk_type_inference() {
        let rows = vec![vec![Value::BigInt(42), Value::string("hello")]];
        let chunk = DataChunk::from_rows(rows);
        assert_eq!(chunk.schema.columns[0].data_type, "bigint");
        assert_eq!(chunk.schema.columns[1].data_type, "string");
    }

    #[test]
    fn flat_property_column_hits_columnar_path() {
        let layout = Arc::new(SlotLayout::from_names(&[
            "p".to_string(),
            "p.age".to_string(),
        ]));
        let rows = vec![
            vec![
                Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
                Value::BigInt(30),
            ],
            vec![
                Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(2)))),
                Value::BigInt(20),
            ],
        ];
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        let expr = Expression::binary(
            Expression::property(Expression::variable("p"), "age"),
            BinaryOperator::GreaterThan,
            Expression::literal(Value::BigInt(28)),
        );
        let results = chunk
            .evaluate_expression(&expr, None)
            .expect("evaluate should succeed");
        assert_eq!(results, vec![Value::Bool(true), Value::Bool(false)]);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn flat_column_promise_holds_for_flat_layout() {
        let layout = Arc::new(SlotLayout::from_names(&[
            "p".to_string(),
            "p.age".to_string(),
            "p.name".to_string(),
        ]));
        let rows = vec![vec![
            Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
            Value::BigInt(30),
            Value::string("Alice"),
        ]];
        let chunk = DataChunk::new_with_layout(rows, layout);
        let expr = Expression::binary(
            Expression::property(Expression::variable("p"), "age"),
            BinaryOperator::GreaterThan,
            Expression::literal(Value::BigInt(28)),
        );
        assert!(chunk.columnar_promise_holds(&expr));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn flat_column_promise_does_not_hold_without_compound_slot() {
        let layout = Arc::new(SlotLayout::from_names(&["p".to_string()]));
        let rows = vec![vec![Value::Vertex(Box::new(Vertex::with_vid(
            VertexId::from_int64(1),
        )))]];
        let chunk = DataChunk::new_with_layout(rows, layout);
        let expr = Expression::property(Expression::variable("p"), "age");
        assert!(!chunk.columnar_promise_holds(&expr));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn flat_column_promise_excludes_unsupported_nodes() {
        let layout = Arc::new(SlotLayout::from_names(&[
            "p".to_string(),
            "p.age".to_string(),
        ]));
        let rows = vec![vec![
            Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
            Value::BigInt(30),
        ]];
        let chunk = DataChunk::new_with_layout(rows, layout);
        let expr = Expression::function(
            "abs".to_string(),
            vec![Expression::property(Expression::variable("p"), "age")],
        );
        assert!(!chunk.columnar_promise_holds(&expr));
    }

    #[test]
    fn columnar_stats_record_hits_and_misses() {
        let stats = Arc::new(crate::query::executor::streaming::runtime::ColumnarStats::new());
        let layout = Arc::new(SlotLayout::from_names(&[
            "p".to_string(),
            "p.age".to_string(),
        ]));
        let rows = vec![vec![
            Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
            Value::BigInt(30),
        ]];
        let mut chunk = DataChunk::new_with_layout(rows, layout).with_columnar_stats(stats.clone());

        let simple = Expression::binary(
            Expression::property(Expression::variable("p"), "age"),
            BinaryOperator::GreaterThan,
            Expression::literal(Value::BigInt(28)),
        );
        chunk
            .evaluate_expression(&simple, None)
            .expect("columnar evaluation should succeed");
        assert_eq!(stats.columnar_hits.load(Ordering::Relaxed), 1);
        assert_eq!(stats.columnar_misses.load(Ordering::Relaxed), 0);
        assert_eq!(stats.hit_rate(), 1.0);

        let complex = Expression::function(
            "abs".to_string(),
            vec![Expression::property(Expression::variable("p"), "age")],
        );
        chunk
            .evaluate_expression(&complex, None)
            .expect("per-row evaluation should succeed");
        assert_eq!(stats.columnar_hits.load(Ordering::Relaxed), 1);
        assert_eq!(stats.columnar_misses.load(Ordering::Relaxed), 1);
        assert!((stats.hit_rate() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn column_cache_is_lazy_and_deferred_after_take_indices() {
        let layout = Arc::new(SlotLayout::from_names(&[
            "p".to_string(),
            "p.age".to_string(),
        ]));
        let rows = vec![
            vec![
                Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
                Value::BigInt(30),
            ],
            vec![
                Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(2)))),
                Value::BigInt(20),
            ],
        ];
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        assert!(chunk.columns.is_none(), "no proactive materialisation");

        let col = chunk.get_column(1).expect("age column");
        assert_eq!(col, vec![Value::BigInt(30), Value::BigInt(20)]);
        assert!(chunk.columns.is_some(), "materialised and cached on demand");

        let mut selected = chunk.take_indices(&[1]);
        assert!(
            selected.columns.is_none(),
            "columnar cache rebuild is deferred after take_indices"
        );
        assert_eq!(
            selected.get_column(1).expect("age column"),
            vec![Value::BigInt(20)]
        );
    }

    #[test]
    fn typed_columns_build_pure_bigint_column() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..100)
            .map(|i| vec![Value::BigInt((i % 1000) as i64)])
            .collect();
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        let bytes = chunk.build_typed_columns();
        let typed = chunk.typed_column(0).expect("typed column built");
        assert!(matches!(typed, TypedColumn::I64(_)), "expected I64 layout");
        assert!(bytes > 0, "typed allocation must be accounted");
        assert_eq!(
            typed.value_at(5),
            Some(Value::BigInt(5)),
            "O(1) indexed materialization"
        );
    }

    #[test]
    fn typed_columns_fallback_on_null_and_mixed_and_string() {
        let layout = Arc::new(SlotLayout::from_names(&[
            "n".to_string(),
            "mixed".to_string(),
            "s".to_string(),
        ]));
        let rows = vec![
            vec![
                Value::Null(crate::core::value::NullType::Null),
                Value::BigInt(1),
                Value::string("a"),
            ],
            vec![Value::BigInt(2), Value::Double(2.0), Value::string("b")],
        ];
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();
        assert!(matches!(
            chunk.typed_column(0),
            Some(TypedColumn::Fallback(_))
        ));
        assert!(matches!(
            chunk.typed_column(1),
            Some(TypedColumn::Fallback(_))
        ));
        assert!(matches!(
            chunk.typed_column(2),
            Some(TypedColumn::Fallback(_))
        ));
    }

    #[test]
    fn typed_columns_survive_take_indices() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::BigInt(i as i64)]).collect();
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();
        let mut selected = chunk.take_indices(&[0, 2, 4]);
        assert!(matches!(
            selected.typed_column(0),
            Some(TypedColumn::I64(_))
        ));
        assert_eq!(
            selected.get_column(0).expect("column"),
            vec![Value::BigInt(0), Value::BigInt(2), Value::BigInt(4)]
        );
    }

    #[test]
    fn typed_eval_matches_value_path_semantics() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..100)
            .map(|i| vec![Value::BigInt((i % 1000) as i64)])
            .collect();
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();

        let expr = Expression::binary(
            Expression::variable("k0"),
            BinaryOperator::GreaterThan,
            Expression::literal(Value::BigInt(500)),
        );
        let typed_result = chunk.evaluate_expression(&expr, None).expect("eval");
        assert_eq!(typed_result.len(), 100);

        let expected: Vec<Value> = (0..100).map(|i| Value::Bool((i % 1000) > 500)).collect();
        assert_eq!(typed_result, expected);
    }

    #[test]
    fn typed_eval_supports_arithmetic_and_cast() {
        let layout = Arc::new(SlotLayout::from_names(&["a".to_string(), "b".to_string()]));
        let rows = vec![
            vec![Value::BigInt(40), Value::BigInt(2)],
            vec![Value::BigInt(10), Value::BigInt(5)],
        ];
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();

        let add = Expression::binary(
            Expression::variable("a"),
            BinaryOperator::Add,
            Expression::variable("b"),
        );
        assert_eq!(
            chunk.evaluate_expression(&add, None).expect("add"),
            vec![Value::BigInt(42), Value::BigInt(15)]
        );

        let cast = Expression::TypeCast {
            expression: Box::new(Expression::variable("a")),
            target_type: crate::core::DataType::Double,
        };
        assert_eq!(
            chunk.evaluate_expression(&cast, None).expect("cast"),
            vec![Value::Double(40.0), Value::Double(10.0)]
        );
    }

    #[test]
    fn typed_columns_disabled_falls_back() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::BigInt(i as i64)]).collect();
        set_typed_columns_enabled(false);
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        let bytes = chunk.build_typed_columns();
        assert_eq!(bytes, 0);
        assert!(chunk.typed_column(0).is_none());
        set_typed_columns_enabled(true);
    }

    #[test]
    fn row_pool_recycles_typed_columns() {
        let pool = RowPool::new(64, 1);
        let col = pool.acquire_typed(TypedKind::I64);
        pool.release_typed(col);
        let col = pool.acquire_typed(TypedKind::I64);
        match col {
            TypedColumn::I64(v) => assert!(v.capacity() >= 64, "recycled capacity"),
            _ => panic!("expected I64 column"),
        }
    }

    #[test]
    fn selection_attachment_and_materialization() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::BigInt(i as i64)]).collect();
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();

        let chunk = chunk.with_selection(vec![0, 2, 4]);
        assert_eq!(chunk.visible_count(), 3);
        assert_eq!(chunk.visible_indices(), vec![0, 2, 4]);
        assert!(chunk.is_visible(2));
        assert!(!chunk.is_visible(1));

        let mut chunk = chunk;
        assert!(chunk.materialize_selection());
        assert_eq!(chunk.visible_count(), 3);
        assert!(chunk.selection().is_none());
        let col = chunk.get_column(0).expect("column");
        assert_eq!(
            col,
            vec![Value::BigInt(0), Value::BigInt(2), Value::BigInt(4)]
        );
        assert!(!chunk.materialize_selection());
    }

    #[test]
    fn selection_preserves_typed_columns_until_materialized() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..6).map(|i| vec![Value::BigInt(i as i64)]).collect();
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();
        let mut chunk = chunk.with_selection(vec![1, 3]);
        assert!(chunk.typed_column(0).is_some(), "typed layout kept");
        chunk.materialize_selection();
        let typed = chunk.typed_column(0).expect("typed layout gathered");
        assert_eq!(typed.to_values(), vec![Value::BigInt(1), Value::BigInt(3)]);
    }

    #[test]
    fn typed_eval_differential_random() {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let n = 1024;

        let layout = Arc::new(SlotLayout::from_names(&["a".to_string(), "b".to_string()]));
        let rows: Vec<Vec<Value>> = (0..n)
            .map(|_| {
                vec![
                    Value::BigInt(rng.gen_range(-1000..1000)),
                    Value::BigInt(rng.gen_range(-1000..1000)),
                ]
            })
            .collect();

        let mut chunk_typed = DataChunk::new_with_layout(rows.clone(), Arc::clone(&layout));
        chunk_typed.build_typed_columns();

        let mut chunk_row = DataChunk::new_with_layout(rows, layout);

        for (op_name, op) in [
            ("==", BinaryOperator::Equal),
            ("!=", BinaryOperator::NotEqual),
            ("<", BinaryOperator::LessThan),
            ("<=", BinaryOperator::LessThanOrEqual),
            (">", BinaryOperator::GreaterThan),
            (">=", BinaryOperator::GreaterThanOrEqual),
        ] {
            let expr =
                Expression::binary(Expression::variable("a"), op, Expression::variable("b"));

            let result_typed = chunk_typed
                .evaluate_expression(&expr, None)
                .expect("typed eval");
            let result_row = chunk_row
                .evaluate_expression(&expr, None)
                .expect("row eval");

            assert_eq!(
                result_typed, result_row,
                "comparison {} mismatch",
                op_name
            );
        }

        for (op_name, op) in [
            ("+", BinaryOperator::Add),
            ("-", BinaryOperator::Subtract),
            ("*", BinaryOperator::Multiply),
        ] {
            let expr = Expression::binary(Expression::variable("a"), op, Expression::variable("b"));

            let result_typed = chunk_typed
                .evaluate_expression(&expr, None)
                .expect("typed eval");
            let result_row = chunk_row
                .evaluate_expression(&expr, None)
                .expect("row eval");

            assert_eq!(
                result_typed, result_row,
                "arithmetic {} mismatch",
                op_name
            );
        }
    }

    #[test]
    fn typed_bool_column_and_or() {
        let layout = Arc::new(SlotLayout::from_names(&["x".to_string(), "y".to_string()]));
        let rows = vec![
            vec![Value::Bool(true), Value::Bool(true)],
            vec![Value::Bool(true), Value::Bool(false)],
            vec![Value::Bool(false), Value::Bool(true)],
            vec![Value::Bool(false), Value::Bool(false)],
        ];
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();

        assert!(
            matches!(chunk.typed_column(0), Some(TypedColumn::Bool(_))),
            "column x should be typed Bool"
        );
        assert!(
            matches!(chunk.typed_column(1), Some(TypedColumn::Bool(_))),
            "column y should be typed Bool"
        );

        let and_expr = Expression::binary(
            Expression::variable("x"),
            BinaryOperator::And,
            Expression::variable("y"),
        );
        let and_result = chunk.evaluate_expression(&and_expr, None).expect("and eval");
        assert_eq!(
            and_result,
            vec![
                Value::Bool(true),
                Value::Bool(false),
                Value::Bool(false),
                Value::Bool(false)
            ]
        );

        let or_expr = Expression::binary(
            Expression::variable("x"),
            BinaryOperator::Or,
            Expression::variable("y"),
        );
        let or_result = chunk.evaluate_expression(&or_expr, None).expect("or eval");
        assert_eq!(
            or_result,
            vec![
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(false)
            ]
        );
    }
