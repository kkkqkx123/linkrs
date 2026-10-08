use super::key::ParamPosition;

/// Parameterized query processor
///
/// Handling the parsing and binding of parameterized queries
pub struct ParameterizedQueryHandler;

impl ParameterizedQueryHandler {
    /// Create a new parametric query processor.
    pub fn new() -> Self {
        Self
    }

    /// Extract the parameter positions from the query.
    ///
    /// # Parameters
    /// - `query`: query text
    ///
    /// # Returns
    /// Parameter Location List
    pub fn extract_params(&self, query: &str) -> Vec<ParamPosition> {
        self.extract_param_matches(query)
            .into_iter()
            .map(|(position, _)| position)
            .collect()
    }

    /// Extract the parameter matches from the query together with their end
    /// offsets. Assignment left-hand sides (`$var = ...`) are excluded, and
    /// `@name` / `$N` occurrences inside string literals (`'...'` / `"..."`,
    /// backslash escapes respected) are ignored so that literal content such
    /// as an e-mail address does not look like a parameter.
    fn extract_param_matches(&self, query: &str) -> Vec<(ParamPosition, usize)> {
        let mut positions = Vec::new();
        let chars: Vec<char> = query.chars().collect();
        let mut i = 0usize;
        let mut idx = 0usize;
        let mut in_string: Option<char> = None;

        while i < chars.len() {
            let c = chars[i];

            if let Some(quote) = in_string {
                if c == '\\' {
                    i = (i + 2).min(chars.len());
                    continue;
                }
                if c == quote {
                    in_string = None;
                }
                i += 1;
                continue;
            }

            match c {
                '\'' | '"' => {
                    in_string = Some(c);
                    i += 1;
                }
                '@' => {
                    let mut j = i + 1;
                    if j < chars.len() && (chars[j].is_ascii_alphabetic() || chars[j] == '_') {
                        j += 1;
                        while j < chars.len()
                            && (chars[j].is_ascii_alphanumeric() || chars[j] == '_')
                        {
                            j += 1;
                        }
                        let name: String = chars[i + 1..j].iter().collect();
                        // Skip the left-hand side of a variable assignment,
                        // e.g. `$result = GO ...` defines a session variable
                        // instead of declaring a named query parameter.
                        let after_match = &query[j..];
                        if !after_match.trim_start().starts_with('=') {
                            positions.push((
                                ParamPosition {
                                    index: idx,
                                    name: Some(name),
                                    position: i,
                                    expected_type: None,
                                },
                                j,
                            ));
                            idx += 1;
                        }
                        i = j;
                    } else {
                        i += 1;
                    }
                }
                '$' => {
                    let mut j = i + 1;
                    let start = j;
                    while j < chars.len() && chars[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j > start {
                        let digits: String = chars[start..j].iter().collect();
                        let after_match = &query[j..];
                        if !after_match.trim_start().starts_with('=') {
                            let parsed = digits.parse::<usize>().unwrap_or(idx);
                            positions.push((
                                ParamPosition {
                                    index: parsed,
                                    name: None,
                                    position: i,
                                    expected_type: None,
                                },
                                j,
                            ));
                            idx += 1;
                        }
                        i = j;
                    } else {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }

        positions
    }

    /// Parameterize the query (replace parameters with placeholders)
    ///
    /// # Parameters
    /// - `query`: query text
    ///
    /// # Returns
    /// (parameterized query, parameter list)
    pub fn parameterize(&self, query: &str) -> (String, Vec<ParamPosition>) {
        let matches = self.extract_param_matches(query);
        let positions = matches
            .iter()
            .map(|(position, _)| position.clone())
            .collect::<Vec<_>>();

        // Replace only the matches that were accepted as parameters so that
        // assignment left-hand sides ($var = ...) stay intact in the template.
        let mut parameterized = String::with_capacity(query.len());
        let mut last_end = 0;
        for (position, end) in matches {
            parameterized.push_str(&query[last_end..position.position]);
            parameterized.push('?');
            last_end = end;
        }
        parameterized.push_str(&query[last_end..]);
        (parameterized, positions)
    }
}

impl Default for ParameterizedQueryHandler {
    fn default() -> Self {
        Self::new()
    }
}
