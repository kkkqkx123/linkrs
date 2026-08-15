use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use colored::Colorize;
use rustyline::completion::{Candidate, Completer};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Helper, Result};

use crate::completion::context::{
    detect_context, get_function_completions, CompletionContext, FunctionEntry, SharedSchemaCache,
};

const GQL_KEYWORDS: &[&str] = &[
    "MATCH",
    "GO",
    "LOOKUP",
    "FETCH",
    "INSERT",
    "UPDATE",
    "DELETE",
    "CREATE",
    "ALTER",
    "DROP",
    "GRANT",
    "REVOKE",
    "RETURN",
    "WHERE",
    "ORDER",
    "BY",
    "LIMIT",
    "SKIP",
    "AND",
    "OR",
    "NOT",
    "IN",
    "AS",
    "WITH",
    "UNWIND",
    "SET",
    "REMOVE",
    "MERGE",
    "OPTIONAL",
    "DISTINCT",
    "UNION",
    "ALL",
    "EXISTS",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "ASC",
    "DESC",
    "TRUE",
    "FALSE",
    "NULL",
    "IS",
    "LIKE",
    "CONTAINS",
    "STARTS",
    "OVER",
    "STEPS",
    "FROM",
    "TO",
    "YIELD",
    "VERTEX",
    "EDGE",
    "VERTICES",
    "EDGES",
    "TAG",
    "TAGS",
    "SPACE",
    "SPACES",
    "INDEX",
    "INDEXES",
    "SHOW",
    "USE",
    "DESCRIBE",
    "EXPLAIN",
    "PROFILE",
    "REBUILD",
    "SUBGRAPH",
    "GROUP",
    "COUNT",
    "SUM",
    "AVG",
    "MAX",
    "MIN",
    "COLLECT",
    "HEAD",
    "TAIL",
    "SIZE",
    "LENGTH",
    "TYPE",
    "PROPERTIES",
    "ID",
    "LABEL",
    "RANK",
    "DATETIME",
    "TIMESTAMP",
    "STRING",
    "INT",
    "INTEGER",
    "FLOAT",
    "DOUBLE",
    "BOOL",
    "BOOLEAN",
    "LIST",
    "MAP",
    "SET",
    "IF",
    "COMMENT",
    "DEFAULT",
    "PARTITION_NUM",
    "REPLICA_FACTOR",
    "VID_TYPE",
    "TTL_DURATION",
    "TTL_COL",
];

const META_COMMANDS: &[&str] = &[
    "\\connect",
    "\\c",
    "\\disconnect",
    "\\conninfo",
    "\\show_spaces",
    "\\l",
    "\\show_tags",
    "\\dt",
    "\\show_edges",
    "\\de",
    "\\show_indexes",
    "\\di",
    "\\show_users",
    "\\du",
    "\\show_functions",
    "\\df",
    "\\describe",
    "\\d",
    "\\describe_edge",
    "\\format",
    "\\pager",
    "\\timing",
    "\\x",
    "\\set",
    "\\unset",
    "\\i",
    "\\ir",
    "\\o",
    "\\!",
    "\\help",
    "\\?",
    "\\version",
    "\\copyright",
    "\\q",
    "\\quit",
    "\\begin",
    "\\commit",
    "\\rollback",
    "\\e",
    "\\p",
    "\\r",
    "\\w",
    "\\history",
];

const FORMAT_VALUES: &[&str] = &["table", "csv", "json", "vertical", "html"];

#[derive(Debug)]
pub struct StringCandidate {
    display: String,
    replacement: String,
}

impl Candidate for StringCandidate {
    fn display(&self) -> &str {
        &self.display
    }

    fn replacement(&self) -> &str {
        &self.replacement
    }
}

#[derive(Debug)]
pub struct GraphDBCompleter {
    keywords: Vec<String>,
    meta_commands: Vec<String>,
    functions: Vec<FunctionEntry>,
    schema_cache: SharedSchemaCache,
    variables: Arc<Mutex<HashMap<String, String>>>,
}

impl Default for GraphDBCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphDBCompleter {
    pub fn new() -> Self {
        Self {
            keywords: GQL_KEYWORDS.iter().map(|s| s.to_string()).collect(),
            meta_commands: META_COMMANDS.iter().map(|s| s.to_string()).collect(),
            functions: get_function_completions(),
            schema_cache: Arc::new(Mutex::new(crate::completion::context::SchemaCache::new())),
            variables: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn set_schema_cache(&mut self, cache: SharedSchemaCache) {
        self.schema_cache = cache;
    }

    pub fn set_variables(&mut self, vars: Arc<Mutex<HashMap<String, String>>>) {
        self.variables = vars;
    }

    pub fn update_variables(&self, vars: HashMap<String, String>) {
        if let Ok(mut v) = self.variables.lock() {
            *v = vars;
        }
    }
}

impl Completer for GraphDBCompleter {
    type Candidate = StringCandidate;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> Result<(usize, Vec<StringCandidate>)> {
        let line_to_pos = &line[..pos];

        if line_to_pos.starts_with('\\') {
            return self.complete_meta(line_to_pos, pos);
        }

        let vars = self.variables.lock().ok();
        let empty_vars = HashMap::new();
        let var_map = vars.as_deref().unwrap_or(&empty_vars);
        let context = detect_context(line, pos, var_map);
        drop(vars);

        match context {
            CompletionContext::Keyword => self.complete_keyword(line_to_pos, pos),
            CompletionContext::TagName => self.complete_tag(line_to_pos, pos),
            CompletionContext::EdgeName => self.complete_edge(line_to_pos, pos),
            CompletionContext::SpaceName => self.complete_space(line_to_pos, pos),
            CompletionContext::FunctionName => self.complete_function(line_to_pos, pos),
            CompletionContext::VariableName => self.complete_variable(line_to_pos, pos),
            CompletionContext::PropertyName => self.complete_keyword(line_to_pos, pos),
            CompletionContext::MetaCommandArg => self.complete_meta_arg(line_to_pos, pos),
        }
    }
}

impl GraphDBCompleter {
    fn complete_meta(
        &self,
        line_to_pos: &str,
        pos: usize,
    ) -> Result<(usize, Vec<StringCandidate>)> {
        let partial = line_to_pos.trim_start_matches('\\');

        let after_cmd = if let Some(space_pos) = partial.find(|c: char| c.is_whitespace()) {
            let cmd = &partial[..space_pos];
            let arg = partial[space_pos..].trim();
            if !arg.is_empty() {
                let cmd_lower = cmd.to_lowercase();
                match cmd_lower.as_str() {
                    "format" => {
                        let completions: Vec<StringCandidate> = FORMAT_VALUES
                            .iter()
                            .filter(|v| v.starts_with(arg))
                            .map(|v| StringCandidate {
                                display: v.to_string(),
                                replacement: v[arg.len()..].to_string(),
                            })
                            .collect();
                        let start = pos - arg.len();
                        return Ok((start, completions));
                    }
                    "connect" | "c" => {
                        return self.complete_space_after_meta(arg, pos);
                    }
                    "describe" | "d" => {
                        return self.complete_tag_after_meta(arg, pos);
                    }
                    "describe_edge" => {
                        return self.complete_edge_after_meta(arg, pos);
                    }
                    _ => {}
                }
            }
            false
        } else {
            false
        };

        if !after_cmd {
            let completions: Vec<StringCandidate> = self
                .meta_commands
                .iter()
                .filter(|cmd| cmd.trim_start_matches('\\').starts_with(partial))
                .map(|cmd| StringCandidate {
                    display: cmd.clone(),
                    replacement: cmd[1..].to_string(),
                })
                .collect();

            let start = if partial.is_empty() {
                pos
            } else {
                pos - partial.len() - 1
            };
            return Ok((start, completions));
        }

        Ok((pos, Vec::new()))
    }

    fn complete_keyword(
        &self,
        line_to_pos: &str,
        pos: usize,
    ) -> Result<(usize, Vec<StringCandidate>)> {
        let last_word = get_last_word(line_to_pos);
        if last_word.is_empty() {
            return Ok((pos, Vec::new()));
        }

        let mut completions: Vec<StringCandidate> = self
            .keywords
            .iter()
            .filter(|kw| kw.starts_with(&last_word.to_uppercase()))
            .map(|kw| StringCandidate {
                display: kw.clone(),
                replacement: kw[last_word.len()..].to_string(),
            })
            .collect();

        let func_completions: Vec<StringCandidate> = self
            .functions
            .iter()
            .filter(|f| f.name.starts_with(&last_word.to_lowercase()))
            .map(|f| StringCandidate {
                display: format!("{}(", f.name),
                replacement: format!("{}(", f.name)[last_word.len()..].to_string(),
            })
            .collect();

        completions.extend(func_completions);

        let start = pos - last_word.len();
        Ok((start, completions))
    }

    fn complete_tag(&self, line_to_pos: &str, pos: usize) -> Result<(usize, Vec<StringCandidate>)> {
        let last_word = get_last_word(line_to_pos);
        let names = self.get_tag_names();
        let completions = filter_names(&names, &last_word);
        let start = pos - last_word.len();
        Ok((start, completions))
    }

    fn complete_edge(
        &self,
        line_to_pos: &str,
        pos: usize,
    ) -> Result<(usize, Vec<StringCandidate>)> {
        let last_word = get_last_word(line_to_pos);
        let names = self.get_edge_names();
        let completions = filter_names(&names, &last_word);
        let start = pos - last_word.len();
        Ok((start, completions))
    }

    fn complete_space(
        &self,
        line_to_pos: &str,
        pos: usize,
    ) -> Result<(usize, Vec<StringCandidate>)> {
        let last_word = get_last_word(line_to_pos);
        let names = self.get_space_names();
        let completions = filter_names(&names, &last_word);
        let start = pos - last_word.len();
        Ok((start, completions))
    }

    fn complete_function(
        &self,
        line_to_pos: &str,
        pos: usize,
    ) -> Result<(usize, Vec<StringCandidate>)> {
        let last_word = get_last_word(line_to_pos);
        if last_word.is_empty() {
            return Ok((pos, Vec::new()));
        }

        let mut completions: Vec<StringCandidate> = self
            .functions
            .iter()
            .filter(|f| f.name.starts_with(&last_word.to_lowercase()))
            .map(|f| StringCandidate {
                display: format!("{}(", f.name),
                replacement: format!("{}(", f.name)[last_word.len()..].to_string(),
            })
            .collect();

        let kw_completions: Vec<StringCandidate> = self
            .keywords
            .iter()
            .filter(|kw| kw.starts_with(&last_word.to_uppercase()))
            .map(|kw| StringCandidate {
                display: kw.clone(),
                replacement: kw[last_word.len()..].to_string(),
            })
            .collect();

        completions.extend(kw_completions);

        let start = pos - last_word.len();
        Ok((start, completions))
    }

    fn complete_variable(
        &self,
        line_to_pos: &str,
        pos: usize,
    ) -> Result<(usize, Vec<StringCandidate>)> {
        let vars = self.variables.lock().ok();
        let empty_vars = HashMap::new();
        let var_map = vars.as_deref().unwrap_or(&empty_vars);

        let after_colon = line_to_pos
            .rfind(':')
            .map(|i| &line_to_pos[i + 1..])
            .unwrap_or("");

        let completions: Vec<StringCandidate> = var_map
            .keys()
            .filter(|k| k.starts_with(after_colon))
            .map(|k| StringCandidate {
                display: k.clone(),
                replacement: k[after_colon.len()..].to_string(),
            })
            .collect();

        let start = pos - after_colon.len();
        Ok((start, completions))
    }

    fn complete_meta_arg(
        &self,
        _line_to_pos: &str,
        pos: usize,
    ) -> Result<(usize, Vec<StringCandidate>)> {
        Ok((pos, Vec::new()))
    }

    fn complete_space_after_meta(
        &self,
        arg: &str,
        pos: usize,
    ) -> Result<(usize, Vec<StringCandidate>)> {
        let names = self.get_space_names();
        let completions = filter_names(&names, arg);
        let start = pos - arg.len();
        Ok((start, completions))
    }

    fn complete_tag_after_meta(
        &self,
        arg: &str,
        pos: usize,
    ) -> Result<(usize, Vec<StringCandidate>)> {
        let names = self.get_tag_names();
        let completions = filter_names(&names, arg);
        let start = pos - arg.len();
        Ok((start, completions))
    }

    fn complete_edge_after_meta(
        &self,
        arg: &str,
        pos: usize,
    ) -> Result<(usize, Vec<StringCandidate>)> {
        let names = self.get_edge_names();
        let completions = filter_names(&names, arg);
        let start = pos - arg.len();
        Ok((start, completions))
    }

    fn get_tag_names(&self) -> Vec<String> {
        self.schema_cache
            .lock()
            .ok()
            .map(|c| c.tag_names())
            .unwrap_or_default()
    }

    fn get_edge_names(&self) -> Vec<String> {
        self.schema_cache
            .lock()
            .ok()
            .map(|c| c.edge_names())
            .unwrap_or_default()
    }

    fn get_space_names(&self) -> Vec<String> {
        self.schema_cache
            .lock()
            .ok()
            .map(|c| c.space_names())
            .unwrap_or_default()
    }
}

impl Hinter for GraphDBCompleter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        if line.is_empty() || pos != line.len() {
            return None;
        }

        if line.starts_with('\\') {
            return None;
        }

        let last_word = get_last_word(line);
        if last_word.is_empty() {
            return None;
        }

        let upper = last_word.to_uppercase();
        let matches: Vec<&String> = self
            .keywords
            .iter()
            .filter(|kw| kw.starts_with(&upper) && kw.len() > upper.len())
            .collect();

        if matches.len() == 1 {
            let hint = &matches[0][last_word.len()..];
            return Some(hint.to_string());
        }

        None
    }
}

impl Highlighter for GraphDBCompleter {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        if line.starts_with('\\') {
            return Cow::Owned(line.cyan().to_string());
        }

        let mut result = String::with_capacity(line.len() + 32);
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut word_start = 0;
        let mut in_comment = false;
        let chars: Vec<char> = line.chars().collect();

        for (i, &ch) in chars.iter().enumerate() {
            if in_comment {
                result.push(ch);
                continue;
            }

            if ch == '-' && !in_single_quote && !in_double_quote && i > 0 && chars[i - 1] == '-' {
                in_comment = true;
                result.push(ch);
                continue;
            }

            if ch == '\'' && !in_double_quote {
                in_single_quote = !in_single_quote;
                result.push(ch);
                continue;
            }

            if ch == '"' && !in_single_quote {
                in_double_quote = !in_double_quote;
                result.push(ch);
                continue;
            }

            if in_single_quote || in_double_quote {
                result.push(ch);
                continue;
            }

            let is_separator = ch.is_whitespace()
                || ch == '('
                || ch == ')'
                || ch == '['
                || ch == ']'
                || ch == '{'
                || ch == '}'
                || ch == ','
                || ch == ':'
                || ch == '='
                || ch == '<'
                || ch == '>'
                || ch == ';';

            if is_separator {
                if word_start < i {
                    let word: String = chars[word_start..i].iter().collect();
                    if is_gql_keyword(&word) {
                        result.push_str(&word.to_uppercase().blue().to_string());
                    } else if word.parse::<f64>().is_ok() {
                        result.push_str(&word.yellow().to_string());
                    } else {
                        result.push_str(&word);
                    }
                }
                result.push(ch);
                word_start = i + ch.len_utf8();
            }
        }

        if word_start < chars.len() {
            let word: String = chars[word_start..].iter().collect();
            if is_gql_keyword(&word) {
                result.push_str(&word.to_uppercase().blue().to_string());
            } else if word.parse::<f64>().is_ok() {
                result.push_str(&word.yellow().to_string());
            } else {
                result.push_str(&word);
            }
        }

        Cow::Owned(result)
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _forced: bool) -> bool {
        true
    }
}

impl Validator for GraphDBCompleter {}

impl Helper for GraphDBCompleter {}

fn is_gql_keyword(word: &str) -> bool {
    let upper = word.to_uppercase();
    GQL_KEYWORDS.contains(&upper.as_str())
}

fn get_last_word(input: &str) -> String {
    let trimmed = input.trim_end();
    if trimmed.is_empty() {
        return String::new();
    }

    let word_start = trimmed
        .char_indices()
        .rev()
        .find(|(_, c)| {
            c.is_whitespace()
                || *c == '('
                || *c == '['
                || *c == '{'
                || *c == ','
                || *c == ':'
                || *c == '='
        })
        .map(|(i, _)| i + 1)
        .unwrap_or(0);

    trimmed[word_start..].to_string()
}

fn filter_names(names: &[String], prefix: &str) -> Vec<StringCandidate> {
    let lower_prefix = prefix.to_lowercase();
    names
        .iter()
        .filter(|n| n.to_lowercase().starts_with(&lower_prefix))
        .map(|n| StringCandidate {
            display: n.clone(),
            replacement: n[prefix.len()..].to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_candidate() {
        let candidate = StringCandidate {
            display: "MATCH".to_string(),
            replacement: "CH".to_string(),
        };
        assert_eq!(candidate.display(), "MATCH");
        assert_eq!(candidate.replacement(), "CH");
    }

    #[test]
    fn test_graphdb_completer_new() {
        let completer = GraphDBCompleter::new();
        assert!(!completer.keywords.is_empty());
        assert!(!completer.meta_commands.is_empty());
        assert!(!completer.functions.is_empty());
    }

    #[test]
    fn test_graphdb_completer_default() {
        let completer: GraphDBCompleter = Default::default();
        assert!(!completer.keywords.is_empty());
        assert!(!completer.meta_commands.is_empty());
    }

    #[test]
    fn test_get_last_word() {
        // When input ends with space, trim_end() removes it, then no separator found
        assert_eq!(get_last_word("MATCH "), "MATCH");
        assert_eq!(get_last_word("MATCH v"), "v");
        assert_eq!(get_last_word("MATCH (v:Pers"), "Pers");
        assert_eq!(get_last_word(""), "");
        assert_eq!(get_last_word("   "), "");
        assert_eq!(get_last_word("RETURN cou"), "cou");
        assert_eq!(get_last_word("WHERE p."), "p.");
    }

    #[test]
    fn test_is_gql_keyword() {
        assert!(is_gql_keyword("MATCH"));
        assert!(is_gql_keyword("match"));
        assert!(is_gql_keyword("RETURN"));
        assert!(is_gql_keyword("WHERE"));
        assert!(!is_gql_keyword("unknown"));
        assert!(!is_gql_keyword("xyz"));
    }

    #[test]
    fn test_filter_names() {
        let names = vec![
            "Person".to_string(),
            "Company".to_string(),
            "Product".to_string(),
        ];

        let filtered = filter_names(&names, "Per");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].display, "Person");

        let filtered_lower = filter_names(&names, "per");
        assert_eq!(filtered_lower.len(), 1);
        assert_eq!(filtered_lower[0].display, "Person");

        let filtered_all = filter_names(&names, "");
        assert_eq!(filtered_all.len(), 3);

        let filtered_none = filter_names(&names, "xyz");
        assert!(filtered_none.is_empty());
    }

    #[test]
    fn test_completer_update_variables() {
        let completer = GraphDBCompleter::new();
        let mut vars = HashMap::new();
        vars.insert("var1".to_string(), "value1".to_string());
        completer.update_variables(vars);

        let locked_vars = completer.variables.lock().expect("Failed to lock");
        assert_eq!(locked_vars.get("var1"), Some(&"value1".to_string()));
    }

    #[test]
    fn test_completer_set_schema_cache() {
        let mut completer = GraphDBCompleter::new();
        let new_cache = Arc::new(Mutex::new(crate::completion::context::SchemaCache::new()));
        completer.set_schema_cache(new_cache.clone());

        let locked = completer.schema_cache.lock().expect("Failed to lock");
        assert!(locked.spaces.is_empty());
    }

    #[test]
    fn test_completer_get_tag_names() {
        let completer = GraphDBCompleter::new();
        let names = completer.get_tag_names();
        assert!(names.is_empty());
    }

    #[test]
    fn test_completer_get_edge_names() {
        let completer = GraphDBCompleter::new();
        let names = completer.get_edge_names();
        assert!(names.is_empty());
    }

    #[test]
    fn test_completer_get_space_names() {
        let completer = GraphDBCompleter::new();
        let names = completer.get_space_names();
        assert!(names.is_empty());
    }

    #[test]
    fn test_highlighter_highlight_meta_command() {
        let completer = GraphDBCompleter::new();
        let highlighted = completer.highlight("\\help", 0);
        assert!(highlighted.contains("\\help"));
    }

    #[test]
    fn test_highlighter_highlight_keywords() {
        let completer = GraphDBCompleter::new();
        let highlighted = completer.highlight("MATCH (v)", 0);
        assert!(highlighted.contains("MATCH"));
    }

    #[test]
    fn test_highlighter_highlight_char() {
        let completer = GraphDBCompleter::new();
        assert!(completer.highlight_char("", 0, false));
    }

    #[test]
    fn test_complete_keyword() {
        let completer = GraphDBCompleter::new();
        let (pos, candidates) = completer
            .complete_keyword("MATC", 4)
            .expect("complete_keyword failed");
        assert_eq!(pos, 0);
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.display() == "MATCH"));
    }

    #[test]
    fn test_complete_keyword_empty() {
        let completer = GraphDBCompleter::new();
        let (pos, candidates) = completer
            .complete_keyword("   ", 3)
            .expect("complete_keyword failed");
        assert_eq!(pos, 3);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_complete_function() {
        let completer = GraphDBCompleter::new();
        let (pos, candidates) = completer
            .complete_function("cou", 3)
            .expect("complete_function failed");
        assert_eq!(pos, 0);
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.display().contains("count")));
    }

    #[test]
    fn test_complete_function_empty() {
        let completer = GraphDBCompleter::new();
        let (pos, candidates) = completer
            .complete_function("   ", 3)
            .expect("complete_function failed");
        assert_eq!(pos, 3);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_complete_variable() {
        let completer = GraphDBCompleter::new();
        let mut vars = HashMap::new();
        vars.insert("myvar".to_string(), "value".to_string());
        completer.update_variables(vars);

        let (_pos, candidates) = completer
            .complete_variable("LIMIT :my", 9)
            .expect("complete_variable failed");
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.display() == "myvar"));
    }

    #[test]
    fn test_complete_variable_empty() {
        let completer = GraphDBCompleter::new();
        let (_pos, candidates) = completer
            .complete_variable("LIMIT :", 7)
            .expect("complete_variable failed");
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_complete_meta_empty() {
        let completer = GraphDBCompleter::new();
        let (_pos, candidates) = completer
            .complete_meta("\\", 1)
            .expect("complete_meta failed");
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.display() == "\\help"));
    }

    #[test]
    fn test_complete_meta_with_partial() {
        let completer = GraphDBCompleter::new();
        let (_pos, candidates) = completer
            .complete_meta("\\he", 3)
            .expect("complete_meta failed");
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.display() == "\\help"));
    }

    #[test]
    fn test_complete_meta_format() {
        let completer = GraphDBCompleter::new();
        let (_pos, candidates) = completer
            .complete_meta("\\format tab", 11)
            .expect("complete_meta failed");
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.display() == "table"));
    }

    #[test]
    fn test_complete_tag() {
        let completer = GraphDBCompleter::new();
        let (_pos, candidates) = completer
            .complete_tag("MATCH (v:Pers", 13)
            .expect("complete_tag failed");
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_complete_edge() {
        let completer = GraphDBCompleter::new();
        let (_pos, candidates) = completer
            .complete_edge("OVER FRI", 8)
            .expect("complete_edge failed");
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_complete_space() {
        let completer = GraphDBCompleter::new();
        let (_pos, candidates) = completer
            .complete_space("USE test", 8)
            .expect("complete_space failed");
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_complete_meta_arg() {
        let completer = GraphDBCompleter::new();
        let (_pos, candidates) = completer
            .complete_meta_arg("test", 4)
            .expect("complete_meta_arg failed");
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_complete_space_after_meta() {
        let completer = GraphDBCompleter::new();
        let (_pos, candidates) = completer
            .complete_space_after_meta("myspace", 7)
            .expect("complete_space_after_meta failed");
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_complete_tag_after_meta() {
        let completer = GraphDBCompleter::new();
        let (_pos, candidates) = completer
            .complete_tag_after_meta("Person", 6)
            .expect("complete_tag_after_meta failed");
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_complete_edge_after_meta() {
        let completer = GraphDBCompleter::new();
        let (_pos, candidates) = completer
            .complete_edge_after_meta("FRIEND", 6)
            .expect("complete_edge_after_meta failed");
        assert!(candidates.is_empty());
    }
}
