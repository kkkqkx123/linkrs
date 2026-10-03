    #[test]
    fn test_abs_int() {
        let func = MathFunction::Abs;
        let result = func
            .execute(&[Value::Int(-5)])
            .expect("Abs Function Failure");
        assert_eq!(result, Value::Int(5));
    }

    #[test]
    fn test_abs_float() {
        let func = MathFunction::Abs;
        let result = func
            .execute(&[Value::Float(-5.5)])
            .expect("Abs Function Failure");
        assert_eq!(result, Value::Float(5.5));
    }

    #[test]
    fn test_sqrt() {
        let func = MathFunction::Sqrt;
        let result = func
            .execute(&[Value::Int(16)])
            .expect("Sqrt function failed to execute");
        assert_eq!(result, Value::Float(4.0));
    }

    #[test]
    fn test_pow() {
        let func = MathFunction::Pow;
        let result = func
            .execute(&[Value::Int(2), Value::Int(3)])
            .expect("Pow Function Execution Failure");
        assert_eq!(result, Value::Float(8.0));
    }

    #[test]
    fn test_sin() {
        let func = MathFunction::Sin;
        let result = func
            .execute(&[Value::Float(0.0)])
            .expect("Sin Function Failure");
        assert_eq!(result, Value::Float(0.0));
    }

    #[test]
    fn test_cos() {
        let func = MathFunction::Cos;
        let result = func
            .execute(&[Value::Float(0.0)])
            .expect("Cos Function Failure");
        assert_eq!(result, Value::Float(1.0));
    }

    #[test]
    fn test_round() {
        let func = MathFunction::Round;
        let result = func
            .execute(&[Value::Float(3.7)])
            .expect("Round Function Failure");
        assert_eq!(result, Value::Float(4.0));
    }

    #[test]
    fn test_ceil() {
        let func = MathFunction::Ceil;
        let result = func
            .execute(&[Value::Float(3.2)])
            .expect("Ceil Function Execution Failure");
        assert_eq!(result, Value::Float(4.0));
    }

    #[test]
    fn test_floor() {
        let func = MathFunction::Floor;
        let result = func
            .execute(&[Value::Float(3.9)])
            .expect("Floor function failed to execute");
        assert_eq!(result, Value::Float(3.0));
    }

    #[test]
    fn test_null_handling() {
        let func = MathFunction::Abs;
        let result = func
            .execute(&[Value::Null(NullType::Null)])
            .expect("Abs function null handling failure");
        assert_eq!(result, Value::Null(NullType::Null));
    }

    #[test]
    fn test_factorial() {
        let func = MathFunction::Factorial;
        let result = func
            .execute(&[Value::Int(5)])
            .expect("factorial should succeed");
        assert_eq!(result, Value::BigInt(120));
    }

    #[test]
    fn test_gamma() {
        let func = MathFunction::Gamma;
        let result = func
            .execute(&[Value::Int(1)])
            .expect("gamma should succeed");
        assert!(matches!(result, Value::Float(_)));
    }

    #[test]
    fn test_negate() {
        let func = MathFunction::Negate;
        let result = func
            .execute(&[Value::Int(5)])
            .expect("negate should succeed");
        assert_eq!(result, Value::Int(-5));
    }

    #[test]
    fn test_even() {
        let func = MathFunction::Even;
        let result = func.execute(&[Value::Int(3)]).expect("even should succeed");
        assert_eq!(result, Value::Int(4));
    }

    #[test]
    fn test_bit_shift_left() {
        let func = MathFunction::BitShiftLeft;
        let result = func
            .execute(&[Value::Int(1), Value::Int(3)])
            .expect("bit_shift_left should succeed");
        assert_eq!(result, Value::Int(8));
    }

    #[test]
    fn test_bit_shift_right() {
        let func = MathFunction::BitShiftRight;
        let result = func
            .execute(&[Value::Int(8), Value::Int(2)])
            .expect("bit_shift_right should succeed");
        assert_eq!(result, Value::Int(2));
    }
