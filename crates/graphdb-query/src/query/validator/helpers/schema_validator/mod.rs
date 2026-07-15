mod constant_evaluator;
mod expression_type_inference;
mod property_validator;
mod schema_auto_creator;
mod schema_lookup;

pub use schema_auto_creator::*;
pub use schema_lookup::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::metadata::SchemaManager;
    use crate::core::types::DataType;
    use crate::core::types::PropertyDef;
    use crate::core::NullType;
    use crate::core::Value;
    use crate::query::validator::validator_trait::ValueType;
    use std::sync::Arc;

    fn create_test_validator() -> SchemaValidator {
        let schema_manager = Arc::new(SchemaManager::new());
        SchemaValidator::new(schema_manager)
    }

    #[test]
    fn test_validate_property_exists_success() {
        let validator = create_test_validator();
        let properties = vec![
            PropertyDef::new("name".to_string(), DataType::String),
            PropertyDef::new("age".to_string(), DataType::Int),
        ];

        assert!(validator
            .validate_property_exists("name", &properties)
            .is_ok());
        assert!(validator
            .validate_property_exists("age", &properties)
            .is_ok());
    }

    #[test]
    fn test_validate_property_exists_failure() {
        let validator = create_test_validator();
        let properties = vec![PropertyDef::new("name".to_string(), DataType::String)];

        let result = validator.validate_property_exists("age", &properties);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("not present"));
    }

    #[test]
    fn test_validate_property_type_success() {
        let validator = create_test_validator();

        assert!(validator
            .validate_property_type(
                "name",
                &DataType::String,
                &Value::String("test".to_string())
            )
            .is_ok());
        assert!(validator
            .validate_property_type("age", &DataType::Int, &Value::Int(25))
            .is_ok());
    }

    #[test]
    fn test_validate_property_type_failure() {
        let validator = create_test_validator();

        let result = validator.validate_property_type(
            "age",
            &DataType::Int,
            &Value::String("test".to_string()),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("Desired type") || err.message.contains("type"));
    }

    #[test]
    fn test_validate_not_null_success() {
        let validator = create_test_validator();
        let prop_def = PropertyDef::new("name".to_string(), DataType::String).with_nullable(false);

        assert!(validator
            .validate_not_null("name", &prop_def, &Value::String("test".to_string()))
            .is_ok());
    }

    #[test]
    fn test_validate_not_null_failure() {
        let validator = create_test_validator();
        let prop_def = PropertyDef::new("name".to_string(), DataType::String).with_nullable(false);

        let result =
            validator.validate_not_null("name", &prop_def, &Value::Null(NullType::default()));
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("cannot be NULL"));
    }

    #[test]
    fn test_fill_default_values() {
        let validator = create_test_validator();
        let properties = vec![
            PropertyDef::new("name".to_string(), DataType::String).with_nullable(false),
            PropertyDef::new("email".to_string(), DataType::String)
                .with_nullable(true)
                .with_default(Some(Value::String("default@example.com".to_string()))),
            PropertyDef::new("age".to_string(), DataType::Int).with_nullable(true),
        ];

        let provided = vec![("name".to_string(), Value::String("John".to_string()))];
        let result = validator
            .fill_default_values(&properties, &provided)
            .expect("Failed to fill default values");

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].0, "name");
        assert_eq!(result[1].0, "email");
        assert_eq!(
            result[1].1,
            Value::String("default@example.com".to_string())
        );
        assert_eq!(result[2].0, "age");
        assert!(matches!(result[2].1, Value::Null(_)));
    }

    #[test]
    fn test_validate_vid_string() {
        let validator = create_test_validator();

        assert!(validator
            .validate_vid(&Value::String("vid1".to_string()), &DataType::String)
            .is_ok());
    }

    #[test]
    fn test_validate_vid_int() {
        let validator = create_test_validator();

        assert!(validator
            .validate_vid(&Value::Int(123), &DataType::Int)
            .is_ok());
    }

    #[test]
    fn test_is_type_compatible() {
        assert!(SchemaValidator::is_type_compatible(
            &DataType::Int,
            &DataType::BigInt
        ));
        assert!(SchemaValidator::is_type_compatible(
            &DataType::BigInt,
            &DataType::Int
        ));

        assert!(SchemaValidator::is_type_compatible(
            &DataType::Float,
            &DataType::Double
        ));

        assert!(SchemaValidator::is_type_compatible(
            &DataType::VID,
            &DataType::String
        ));
        assert!(SchemaValidator::is_type_compatible(
            &DataType::VID,
            &DataType::Int
        ));

        assert!(!SchemaValidator::is_type_compatible(
            &DataType::Int,
            &DataType::String
        ));
        assert!(!SchemaValidator::is_type_compatible(
            &DataType::Bool,
            &DataType::Int
        ));
    }

    #[test]
    fn test_data_type_to_value_type() {
        assert!(matches!(
            SchemaValidator::data_type_to_value_type(&DataType::Bool),
            ValueType::Bool
        ));
        assert!(matches!(
            SchemaValidator::data_type_to_value_type(&DataType::Int),
            ValueType::Int
        ));
        assert!(matches!(
            SchemaValidator::data_type_to_value_type(&DataType::String),
            ValueType::String
        ));
    }
}
