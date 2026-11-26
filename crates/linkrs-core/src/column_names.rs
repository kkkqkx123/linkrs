//! Column-name matching shared by import paths and result-shape discovery.

/// Locate a column by case-insensitive name among several aliases.
pub fn find_alias_index(columns: &[String], aliases: &[&str]) -> Option<usize> {
    columns.iter().position(|column| {
        aliases
            .iter()
            .any(|alias| column.eq_ignore_ascii_case(alias))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_match_is_case_insensitive_and_first_wins() {
        let columns = vec!["Id".to_string(), "vertex_id".to_string()];
        assert_eq!(
            find_alias_index(&columns, &["vid", "id", "vertex_id"]),
            Some(0)
        );
        assert_eq!(find_alias_index(&columns, &["missing"]), None);
    }
}
