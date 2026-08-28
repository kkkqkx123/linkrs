use crate::executor::expression::ExpressionError;
use graphdb_core::value::NullType;
use graphdb_core::Value;

pub fn execute_st_astext(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => {
            let wkt = geo.to_wkt();
            Ok(Value::string(wkt))
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
            Ok(Value::string(json_str))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_asgeojson function requires geography type",
        )),
    }
}
