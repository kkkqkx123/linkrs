use serde_json::{json, Value};

pub fn parse_point_id(id: &str) -> Result<u64, &str> {
    id.parse::<u64>().map_err(|_| id)
}

pub fn point_id_to_json(id: &str) -> Value {
    match parse_point_id(id) {
        Ok(num) => json!(num),
        Err(s) => json!(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_point_id_numeric() {
        let result = parse_point_id("42");
        assert_eq!(result, Ok(42));
    }

    #[test]
    fn test_parse_point_id_string() {
        let result = parse_point_id("abc-123");
        assert_eq!(result, Err("abc-123"));
    }

    #[test]
    fn test_point_id_to_json_numeric() {
        let result = point_id_to_json("42");
        assert_eq!(result, json!(42));
    }

    #[test]
    fn test_point_id_to_json_string() {
        let result = point_id_to_json("uuid-abc");
        assert_eq!(result, json!("uuid-abc"));
    }

    #[test]
    fn test_point_id_to_json_large_number_as_string() {
        // Numbers too large for u64 are treated as strings
        let result = point_id_to_json("999999999999999999999999");
        assert_eq!(result, json!("999999999999999999999999"));
    }
}
