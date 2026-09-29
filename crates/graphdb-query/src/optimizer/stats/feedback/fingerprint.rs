//! Fingerprint recognition module
//!
//! Provide functions for query normalization and fingerprint generation.
//! Refer to the implementation of the pg_stat_statements module in PostgreSQL.

/// Query on fingerprint normalization
///
/// Single source: delegates to `graphdb_metrics::normalize_query_text` so
/// feedback keys, slow-query patterns, and plan-shape grouping share one
/// normalization. Lowercases, collapses whitespace, replaces literals
/// with `?`.
///
/// # Example
/// ```
/// use graphdb_query::optimizer::stats::feedback::fingerprint::normalize_query;
///
/// let query = "SELECT * FROM users WHERE id = 123";
/// let normalized = normalize_query(query);
/// assert!(normalized.contains("?"));
/// ```
pub fn normalize_query(query: &str) -> String {
    graphdb_metrics::aggregated_stats::normalize_query_text(query)
}

/// Generate a query fingerprint
///
/// Generate a unique fingerprint based on the normalized query string.
/// Use the FNV-1a hash algorithm.
///
/// # Examples
/// ```
/// use graphdb_query::optimizer::stats::feedback::fingerprint::generate_query_fingerprint;
///
/// let query1 = "SELECT * FROM users WHERE id = 1";
/// let query2 = "SELECT * FROM users WHERE id = 2";
/// let fp1 = generate_query_fingerprint(query1);
/// let fp2 = generate_query_fingerprint(query2);
/// // Different queries with the same structure should have the same "fingerprint"
/// // (i.e., the same result when analyzed using a specific algorithm or method).
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
        assert!(normalized1.contains("?"));
        assert!(normalized1.starts_with("select * from users where"));

        let query2 = "  SELECT   id  FROM   t   WHERE  x = 100  ";
        let normalized2 = normalize_query(query2);
        assert!(normalized2.contains("?"));
        assert!(normalized2.starts_with("select id from t where"));
    }

    #[test]
    fn test_normalize_query_with_escaped_quotes() {
        let query = "SELECT * FROM t WHERE name = 'O''Brien'";
        let normalized = normalize_query(query);
        assert!(normalized.contains("?"));
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
