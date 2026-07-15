use crate::core::value::geography::{Geography, GeographyValue};
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;

pub fn execute_st_point(args: &[Value]) -> Result<Value, ExpressionError> {
    let (lon, lat) = match (&args[0], &args[1]) {
        (Value::Float(lon), Value::Float(lat)) => (*lon as f64, *lat as f64),
        (Value::Double(lon), Value::Double(lat)) => (*lon, *lat),
        (Value::SmallInt(lon), Value::SmallInt(lat)) => (*lon as f64, *lat as f64),
        (Value::Int(lon), Value::Int(lat)) => (*lon as f64, *lat as f64),
        (Value::BigInt(lon), Value::BigInt(lat)) => (*lon as f64, *lat as f64),
        (Value::Float(lon), Value::Double(lat)) => (*lon as f64, *lat),
        (Value::Double(lon), Value::Float(lat)) => (*lon, *lat as f64),
        (Value::Null(_), _) | (_, Value::Null(_)) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "The st_point function takes numeric arguments",
            ))
        }
    };

    let geo = Geography::Point(GeographyValue::new(lat, lon));
    Ok(Value::Geography(geo))
}

pub fn execute_st_geogfromtext(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(wkt) => match Geography::from_wkt(wkt) {
            Ok(geo) => Ok(Value::Geography(geo)),
            Err(e) => Err(ExpressionError::type_error(format!(
                "Failed to parse WKT: {}",
                e
            ))),
        },
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_geogfromtext function takes string arguments",
        )),
    }
}

pub fn execute_st_geomfromgeojson(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(json_str) => match Geography::from_geojson_string(json_str) {
            Ok(geo) => Ok(Value::Geography(geo)),
            Err(e) => Err(ExpressionError::type_error(format!(
                "Invalid GeoJSON: {}",
                e
            ))),
        },
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_geomfromgeojson function requires string argument",
        )),
    }
}
