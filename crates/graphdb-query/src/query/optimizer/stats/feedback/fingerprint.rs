//! Fingerprint recognition module
//!
//! Provide functions for query normalization and fingerprint generation.
//! Refer to the implementation of the pg_stat_statements module in PostgreSQL.

/// Query on fingerprint normalization
///
/// Normalize the query string into a standard format for generating a query fingerprint.
/// Refer to the implementation of the pg_stat_statements module in PostgreSQL.
///
/// # Normalization Rules
/// Remove the spaces at the beginning and end.
/// 2. Replace multiple blank characters with a single space.
/// 3. 将字符串常量替换为占位符($1, $2, ...)
/// 4. Replace the numeric constants with placeholders.
/// 5. Convert all text to lowercase.
///
/// # Example
/// ```
/// use graphdb::query::optimizer::stats::feedback::fingerprint::normalize_query;
///
/// let query = "SELECT * FROM users WHERE id = 123";
/// let normalized = normalize_query(query);
/// assert_eq!(normalized, "select * from users where id = $1");
/// ```
pub fn normalize_query(query: &str) -> String {
    // Remove the leading and trailing spaces.
    let trimmed = query.trim();

    // 2. Replace multiple whitespace characters with a single space.
    let normalized_whitespace = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");

    // 3. Replace the string constants with placeholders.
    let mut result = String::new();
    let mut in_string = false;
    let mut param_count = 0;

    let chars: Vec<char> = normalized_whitespace.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if !in_string {
            // Check if the string starts with...
            if c == '\'' || c == '"' {
                in_string = true;
                param_count += 1;
                result.push_str(&format!("${}", param_count));
                // Skip until the end of the string.
                let string_char = c;
                i += 1;
                while i < chars.len() {
                    if chars[i] == string_char {
                        // Check whether it is an escape sequence.
                        if i + 1 < chars.len() && chars[i + 1] == string_char {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
            } else if c.is_ascii_digit() {
                // Replace the numeric constants with placeholders.
                param_count += 1;
                result.push_str(&format!("${}", param_count));
                // Skip consecutive numbers.
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                continue;
            } else {
                // Convert all text to lowercase.
                result.push(c.to_ascii_lowercase());
            }
        }

        i += 1;
    }

    result
}

/// Generate a query fingerprint
///
/// Generate a unique fingerprint based on the normalized query string.
/// Use the FNV-1a hash algorithm.
///
/// # Examples
/// ```
/// use graphdb::query::optimizer::stats::feedback::fingerprint::generate_query_fingerprint;
///
/// let query1 = "SELECT * FROM users WHERE id = 1";
/// let query2 = "SELECT * FROM users WHERE id = 2";
/// let fp1 = generate_query_fingerprint(query1);
/// let fp2 = generate_query_fingerprint(query2);
/// Different queries with the same structure should have the same “fingerprint” (i.e., the same result when analyzed using a specific algorithm or method).
/// assert_eq!(fp1, fp2);
/// ```
pub fn generate_query_fingerprint(query: &str) -> String {
    let normalized = normalize_query(query);
    // Use the simple FNV-1a hashing algorithm.
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in normalized.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    format!("{:016x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_query() {
        let query1 = "SELECT * FROM users WHERE age > 25 AND name = 'John'";
        let normalized1 = normalize_query(query1);
        assert!(normalized1.contains("$1")); // The numeric constants have been replaced.
        assert!(normalized1.contains("$2")); // The string constant has been replaced.
        assert!(normalized1.starts_with("select * from users where"));

        let query2 = "  SELECT   id  FROM   t   WHERE  x = 100  ";
        let normalized2 = normalize_query(query2);
        assert_eq!(normalized2, "select id from t where x = $1");
    }

    #[test]
    fn test_normalize_query_with_escaped_quotes() {
        let query = "SELECT * FROM t WHERE name = 'O''Brien'";
        let normalized = normalize_query(query);
        assert!(normalized.contains("$1"));
        assert!(normalized.starts_with("select * from t where"));
    }

    #[test]
    fn test_generate_query_fingerprint() {
        let query1 = "SELECT * FROM users WHERE id = 1";
        let query2 = "SELECT * FROM users WHERE id = 2";
        let fp1 = generate_query_fingerprint(query1);
        let fp2 = generate_query_fingerprint(query2);
        // Different queries with the same structure should have the same “fingerprint” (i.e., the same set of characteristics that identify them as belonging to the same category).
        assert_eq!(fp1, fp2);

        // Queries with different structures should have different “fingerprints” (unique identifiers or characteristics that distinguish them from each other).
        let query3 = "SELECT * FROM orders WHERE id = 1";
        let fp3 = generate_query_fingerprint(query3);
        assert_ne!(fp1, fp3);
    }
}
