use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;

pub fn execute_st_astext(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => {
            let wkt = geo.to_wkt();
            Ok(Value::String(wkt))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_astext function requires the geographic type",
        )),
    }
}

pub fn execute_st_asgeojson(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => {
            let json_str = geo.to_geojson_string();
            Ok(Value::String(json_str))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_asgeojson function requires geography type",
        )),
    }
}
