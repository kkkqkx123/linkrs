use super::*;

#[test]
fn test_count_accumulate() {
    let mut acc = AggregateAccumulator::Count(0);
    acc.accumulate(&Value::Int(1));
    acc.accumulate(&Value::Int(2));
    acc.accumulate(&Value::Null(NullType::Null));
    assert_eq!(acc.finalize(), Value::BigInt(2));
}

#[test]
fn test_sum_accumulate() {
    let mut acc = AggregateAccumulator::Sum(0.0);
    acc.accumulate(&Value::Int(1));
    acc.accumulate(&Value::BigInt(2));
    acc.accumulate(&Value::Double(3.5));
    acc.accumulate(&Value::Null(NullType::Null));
    assert!(
        (match acc.finalize() {
            Value::Double(d) => (d - 6.5).abs() < 1e-10,
            _ => false,
        })
    );
}

#[test]
fn test_min_max_typed_compare() {
    // Type-aware comparison: Int 9 < Int 10 despite "10" < "9" as strings.
    let mut min_acc = AggregateAccumulator::Min(None);
    let mut max_acc = AggregateAccumulator::Max(None);
    for v in [Value::Int(10), Value::Int(9), Value::Int(11)] {
        min_acc.accumulate(&v);
        max_acc.accumulate(&v);
    }
    assert_eq!(min_acc.finalize(), Value::Int(9));
    assert_eq!(max_acc.finalize(), Value::Int(11));
}

#[test]
fn test_avg_accumulate() {
    let mut acc = AggregateAccumulator::Avg { sum: 0.0, count: 0 };
    acc.accumulate(&Value::Int(2));
    acc.accumulate(&Value::Int(4));
    acc.accumulate(&Value::Int(6));
    assert!(
        (match acc.finalize() {
            Value::Double(d) => (d - 4.0).abs() < 1e-10,
            _ => false,
        })
    );
}

#[test]
fn test_merge_counts() {
    let mut a = AggregateAccumulator::Count(3);
    let b = AggregateAccumulator::Count(5);
    a.merge(&b);
    assert_eq!(a.finalize(), Value::BigInt(8));
}

#[test]
fn test_merge_avg() {
    let mut a = AggregateAccumulator::Avg {
        sum: 10.0,
        count: 3,
    };
    let b = AggregateAccumulator::Avg {
        sum: 20.0,
        count: 2,
    };
    a.merge(&b);
    assert!(
        (match a.finalize() {
            Value::Double(d) => (d - 6.0).abs() < 1e-10,
            _ => false,
        })
    );
}

#[test]
fn test_empty_returns_null() {
    let min = AggregateAccumulator::Min(None);
    let max = AggregateAccumulator::Max(None);
    let avg = AggregateAccumulator::Avg { sum: 0.0, count: 0 };
    assert_eq!(min.finalize(), Value::Null(NullType::Null));
    assert_eq!(max.finalize(), Value::Null(NullType::Null));
    assert_eq!(avg.finalize(), Value::Null(NullType::Null));
}

#[test]
fn test_collect() {
    let mut acc = AggregateAccumulator::Collect(Vec::new());
    acc.accumulate(&Value::Int(1));
    acc.accumulate(&Value::Null(NullType::Null));
    acc.accumulate(&Value::Int(2));
    match acc.finalize() {
        Value::List(l) => {
            assert_eq!(l.values, vec![Value::Int(1), Value::Int(2)]);
        }
        other => panic!("expected list, got {:?}", other),
    }
}

#[test]
fn test_collect_set() {
    let mut acc = AggregateAccumulator::CollectSet(HashSet::new());
    acc.accumulate(&Value::Int(1));
    acc.accumulate(&Value::Int(1));
    acc.accumulate(&Value::Int(2));
    match acc.finalize() {
        Value::Set(s) => assert_eq!(s.len(), 2),
        other => panic!("expected set, got {:?}", other),
    }
}

#[test]
fn test_percentile_and_median() {
    let mut p = AggregateAccumulator::Percentile {
        values: Vec::new(),
        percentile: 50.0,
    };
    for v in [Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)] {
        p.accumulate(&v);
    }
    assert!(
        (match p.finalize() {
            Value::Double(d) => (d - 2.5).abs() < 1e-9,
            _ => false,
        })
    );

    let mut m = AggregateAccumulator::Median(Vec::new());
    for v in [Value::Int(1), Value::Int(2), Value::Int(3)] {
        m.accumulate(&v);
    }
    assert!(
        (match m.finalize() {
            Value::Double(d) => (d - 2.0).abs() < 1e-9,
            _ => false,
        })
    );
}

#[test]
fn test_std_variance_family() {
    let mut std = AggregateAccumulator::Std {
        n: 0,
        mean: 0.0,
        m2: 0.0,
    };
    for v in [Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)] {
        std.accumulate(&v);
    }
    // Population std of [1,2,3,4] = sqrt(1.25)
    assert!(
        (match std.finalize() {
            Value::Double(d) => (d - 1.25f64.sqrt()).abs() < 1e-9,
            _ => false,
        })
    );

    let mut samp = AggregateAccumulator::StddevSamp {
        n: 0,
        mean: 0.0,
        m2: 0.0,
    };
    for v in [Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)] {
        samp.accumulate(&v);
    }
    // Sample std of [1,2,3,4] = sqrt(5/3)
    assert!(
        (match samp.finalize() {
            Value::Double(d) => (d - (5.0f64 / 3.0).sqrt()).abs() < 1e-9,
            _ => false,
        })
    );

    let mut var = AggregateAccumulator::Variance {
        n: 0,
        mean: 0.0,
        m2: 0.0,
    };
    for v in [Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)] {
        var.accumulate(&v);
    }
    assert!(
        (match var.finalize() {
            Value::Double(d) => (d - 1.25).abs() < 1e-9,
            _ => false,
        })
    );

    let empty = AggregateAccumulator::StddevSamp {
        n: 0,
        mean: 0.0,
        m2: 0.0,
    };
    assert_eq!(empty.finalize(), Value::Null(NullType::Null));
}

#[test]
fn test_welford_merge_matches_single_pass() {
    let mut a = AggregateAccumulator::Std {
        n: 0,
        mean: 0.0,
        m2: 0.0,
    };
    let mut b = AggregateAccumulator::Std {
        n: 0,
        mean: 0.0,
        m2: 0.0,
    };
    for v in [Value::Int(1), Value::Int(3)] {
        a.accumulate(&v);
    }
    for v in [Value::Int(2), Value::Int(4), Value::Int(5)] {
        b.accumulate(&v);
    }
    let mut merged = AggregateAccumulator::Std {
        n: 0,
        mean: 0.0,
        m2: 0.0,
    };
    let mut single = AggregateAccumulator::Std {
        n: 0,
        mean: 0.0,
        m2: 0.0,
    };
    merged.merge(&a);
    merged.merge(&b);
    for v in [
        Value::Int(1),
        Value::Int(3),
        Value::Int(2),
        Value::Int(4),
        Value::Int(5),
    ] {
        single.accumulate(&v);
    }
    let (Value::Double(md), Value::Double(sd)) = (merged.finalize(), single.finalize()) else {
        panic!("expected doubles");
    };
    assert!((md - sd).abs() < 1e-9);
}

#[test]
fn test_product() {
    let mut acc = AggregateAccumulator::Product(None);
    acc.accumulate(&Value::Int(2));
    acc.accumulate(&Value::Int(3));
    acc.accumulate(&Value::Null(NullType::Null));
    assert_eq!(acc.finalize(), Value::BigInt(6));
    let empty = AggregateAccumulator::Product(None);
    assert_eq!(empty.finalize(), Value::BigInt(0));
}

#[test]
fn test_bit_bool() {
    let mut and = AggregateAccumulator::BitAnd(None);
    and.accumulate(&Value::BigInt(0b1100));
    and.accumulate(&Value::BigInt(0b1010));
    assert_eq!(and.finalize(), Value::BigInt(0b1000));

    let mut band = AggregateAccumulator::BoolAnd(None);
    band.accumulate(&Value::Bool(true));
    band.accumulate(&Value::Bool(true));
    assert_eq!(band.finalize(), Value::Bool(true));
    band.accumulate(&Value::Bool(false));
    assert_eq!(band.finalize(), Value::Bool(false));

    let mut bor = AggregateAccumulator::BoolOr(None);
    bor.accumulate(&Value::Bool(false));
    bor.accumulate(&Value::Bool(true));
    assert_eq!(bor.finalize(), Value::Bool(true));
}

#[test]
fn test_group_concat() {
    let mut acc = AggregateAccumulator::GroupConcat {
        parts: Vec::new(),
        separator: ", ".to_string(),
    };
    acc.accumulate(&Value::string("a"));
    acc.accumulate(&Value::string("b"));
    assert_eq!(acc.finalize(), Value::string("a, b"));
}

#[test]
fn test_vec_sum_avg() {
    let mut sum = AggregateAccumulator::VecSum(None);
    sum.accumulate(&Value::vector(vec![1.0, 2.0]));
    sum.accumulate(&Value::vector(vec![3.0, 4.0]));
    match sum.finalize() {
        Value::Vector(v) => {
            assert_eq!(v.to_dense(), vec![4.0, 6.0]);
        }
        other => panic!("expected vector, got {:?}", other),
    }

    let mut avg = AggregateAccumulator::VecAvg {
        sum: Vec::new(),
        count: 0,
    };
    avg.accumulate(&Value::vector(vec![1.0, 3.0]));
    avg.accumulate(&Value::vector(vec![3.0, 5.0]));
    match avg.finalize() {
        Value::Vector(v) => {
            assert_eq!(v.to_dense(), vec![2.0, 4.0]);
        }
        other => panic!("expected vector, got {:?}", other),
    }
}

#[test]
fn test_for_function_mapping_covers_all_variants() {
    let funcs = [
        (AggregateFunction::Count, vec![]),
        (
            AggregateFunction::Sum,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::Min,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::Max,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::Avg,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::Collect,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::CollectSet,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::Percentile,
            vec![
                Expression::Variable("x".to_string()),
                Expression::Literal(Value::Double(50.0)),
            ],
        ),
        (
            AggregateFunction::Std,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::StddevPop,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::StddevSamp,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::Variance,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::Product,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::PercentileCont,
            vec![
                Expression::Variable("x".to_string()),
                Expression::Literal(Value::Double(50.0)),
            ],
        ),
        (
            AggregateFunction::Median,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::Mode,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::BitAnd,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::BitOr,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::BoolAnd,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::BoolOr,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::GroupConcat,
            vec![
                Expression::Variable("x".to_string()),
                Expression::Literal(Value::string(",")),
            ],
        ),
        (
            AggregateFunction::GroupConcatWithOrder,
            vec![
                Expression::Variable("x".to_string()),
                Expression::Literal(Value::string(",")),
            ],
        ),
        (
            AggregateFunction::VecSum,
            vec![Expression::Variable("x".to_string())],
        ),
        (
            AggregateFunction::VecAvg,
            vec![Expression::Variable("x".to_string())],
        ),
    ];
    for (func, args) in &funcs {
        assert!(
            AggregateAccumulator::for_function(func, args).is_some(),
            "missing accumulator for {:?}",
            func
        );
    }
}

#[test]
fn test_accumulator_to_value_roundtrip() {
    let acc = AggregateAccumulator::Avg {
        sum: 15.0,
        count: 3,
    };
    let v = accumulator_to_value(&acc);
    let result = finalize_accumulator_value(&AggregateFunction::Avg, &v, None);
    assert!(
        (match result {
            Value::Double(d) => (d - 5.0).abs() < 1e-10,
            _ => false,
        })
    );
}

#[test]
fn test_decode_partial_roundtrip_all_variants() {
    let cases: Vec<(AggregateFunction, Vec<Expression>, AggregateAccumulator)> = vec![
        (
            AggregateFunction::Count,
            vec![],
            AggregateAccumulator::Count(7),
        ),
        (
            AggregateFunction::Sum,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::Sum(3.5),
        ),
        (
            AggregateFunction::Min,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::Min(Some(Value::Int(3))),
        ),
        (
            AggregateFunction::Max,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::Max(Some(Value::Int(9))),
        ),
        (
            AggregateFunction::Avg,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::Avg { sum: 4.0, count: 2 },
        ),
        (
            AggregateFunction::Collect,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::Collect(vec![Value::Int(1), Value::Int(2)]),
        ),
        (
            AggregateFunction::CollectSet,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::CollectSet([Value::Int(1), Value::Int(2)].into_iter().collect()),
        ),
        (
            AggregateFunction::Median,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::Median(vec![1.0, 2.0]),
        ),
        (
            AggregateFunction::Mode,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::Mode(vec![Value::Int(1), Value::Int(1)]),
        ),
        (
            AggregateFunction::Std,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::Std {
                n: 3,
                mean: 2.0,
                m2: 2.0,
            },
        ),
        (
            AggregateFunction::StddevSamp,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::StddevSamp {
                n: 4,
                mean: 1.0,
                m2: 1.5,
            },
        ),
        (
            AggregateFunction::Variance,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::Variance {
                n: 2,
                mean: 0.5,
                m2: 0.25,
            },
        ),
        (
            AggregateFunction::Product,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::Product(Some(6.0)),
        ),
        (
            AggregateFunction::BitAnd,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::BitAnd(Some(4)),
        ),
        (
            AggregateFunction::BoolAnd,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::BoolAnd(Some(true)),
        ),
        (
            AggregateFunction::GroupConcat,
            vec![
                Expression::Variable("x".to_string()),
                Expression::Literal(Value::string(";")),
            ],
            AggregateAccumulator::GroupConcat {
                parts: vec!["a".to_string(), "b".to_string()],
                separator: ";".to_string(),
            },
        ),
        (
            AggregateFunction::VecSum,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::VecSum(Some(vec![1.0, 2.0])),
        ),
        (
            AggregateFunction::VecAvg,
            vec![Expression::Variable("x".to_string())],
            AggregateAccumulator::VecAvg {
                sum: vec![2.0, 4.0],
                count: 2,
            },
        ),
    ];
    for (func, args, acc) in cases {
        let encoded = accumulator_to_value(&acc);
        let decoded =
            decode_partial_with_args(&func, &args, &encoded).expect("decode should succeed");
        let mut merged = AggregateAccumulator::for_function(&func, &args).unwrap();
        merged.merge(&decoded);
        assert_eq!(
            merged.finalize(),
            acc.finalize(),
            "roundtrip mismatch for {:?}",
            func
        );
    }
}
