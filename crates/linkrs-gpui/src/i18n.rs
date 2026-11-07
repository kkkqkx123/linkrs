//! Interface message catalogue for the desktop client.
//!
//! The JSON files in `messages/` are the only place holding non-English
//! text, so Rust sources stay English-only. Lookups fall back to English
//! and then to the key itself, so a missing entry never blanks the UI.
//!
//! Catalogue rules: every key is namespaced (`console.run`, never a bare
//! word or sentence), so backend column names, SQL fragments, log lines
//! and test strings stay out of the JSON files. Only `t`/`tf`/`tf_args`
//! lookups render user text; anything else (SQL, paths, numbers, JSON
//! examples, protocol fields) is never translated.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::settings::Locale;
use crate::state::AppState;

static EN: LazyLock<HashMap<String, String>> =
    LazyLock::new(|| load(include_str!("../messages/en.json")));
static ZH: LazyLock<HashMap<String, String>> =
    LazyLock::new(|| load(include_str!("../messages/zh.json")));

fn load(raw: &str) -> HashMap<String, String> {
    serde_json::from_str(raw).expect("bundled message catalogue parses")
}

/// Interface text for `key` in `locale`. Unknown keys surface as-is so gaps
/// stay visible instead of rendering blank controls.
pub fn t(key: &str, locale: Locale) -> &str {
    match locale {
        Locale::Zh => ZH.get(key).or_else(|| EN.get(key)),
        Locale::En => EN.get(key),
    }
    .map(String::as_str)
    .unwrap_or(key)
}

/// Current interface locale, mirrored in the global state.
pub fn locale(cx: &gpui_kit::App) -> Locale {
    AppState::global(cx).read(cx).locale
}

/// Formatted lookup: replaces the first `{}` placeholder with `arg`.
/// Multi-placeholder templates use [`tf_args`] so every slot is filled in
/// order; chaining `replacen` at call sites is not allowed because a
/// skipped slot silently renders a raw `{}`.
pub fn tf(key: &str, locale: Locale, arg: impl std::fmt::Display) -> String {
    tf_args(key, locale, &[&arg])
}

/// Formatted lookup for templates with any number of `{}` placeholders.
/// Slots fill left to right; surplus template slots keep their `{}` so a
/// mismatch stays visible instead of shifting arguments.
pub fn tf_args(key: &str, locale: Locale, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = t(key, locale).to_string();
    for arg in args {
        if !out.contains("{}") {
            break;
        }
        out = out.replacen("{}", &arg.to_string(), 1);
    }
    out
}

/// Persist the language and mirror it into the observable global state so
/// observing views re-render; the toggle host notifies its own view too.
pub fn set_locale(locale: Locale, cx: &mut gpui_kit::App) {
    crate::settings::update(cx, |settings| settings.locale = locale);
    AppState::global(cx).update(cx, |state, cx| {
        state.set_locale(locale, cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogues_parse() {
        assert!(!EN.is_empty());
        assert!(!ZH.is_empty());
    }

    #[test]
    fn every_chinese_key_exists_in_english() {
        for key in ZH.keys() {
            assert!(EN.contains_key(key), "orphan key {key}");
        }
    }

    #[test]
    fn unknown_keys_surface_as_is() {
        assert_eq!(
            t("definitely.missing.key", Locale::En),
            "definitely.missing.key"
        );
        assert_eq!(
            t("definitely.missing.key", Locale::Zh),
            "definitely.missing.key"
        );
    }

    #[test]
    fn multi_arg_templates_fill_in_order() {
        let single = tf_args("console.rowsMs", Locale::En, &[&3, &12]);
        assert_eq!(single, "3 rows · 12 ms");
        let triple = tf_args(
            "console.sentGraph",
            Locale::En,
            &[&1, &2, &" (truncated)"],
        );
        assert_eq!(triple, "Sent 1 nodes and 2 edges to the graph (truncated)");
        // Single-arg helper keeps its behavior through the shared path.
        assert_eq!(tf("console.rowsMs", Locale::En, 3), "3 rows · {} ms");
        // Chinese templates fill the same slots in order.
        let zh = tf_args("console.rowsMs", Locale::Zh, &[&3, &12]);
        assert_eq!(zh, "3 行 · 12 ms");
    }

    #[test]
    fn catalogue_keys_stay_namespaced() {
        for key in EN.keys() {
            assert!(key.contains('.'), "catalogue key without namespace: {key}");
            let namespace = key.split('.').next().unwrap_or_default();
            assert!(
                [
                    "common",
                    "language",
                    "login",
                    "nav",
                    "header",
                    "status",
                    "console",
                    "graph",
                    "config",
                    "monitoring",
                    "batch",
                    "transfer",
                    "dataBrowser",
                    "schema",
                    "fulltext",
                    "functions",
                    "migration",
                    "sessions",
                    "sync",
                    "transactions",
                    "users",
                    "vector",
                    "filter",
                ]
                .contains(&namespace),
                "unknown catalogue namespace: {key}"
            );
        }
    }

    #[test]
    fn stage_one_keys_exist_in_both_locales() {
        for key in [
            "login.title",
            "login.connect",
            "status.connected",
            "nav.query",
            "console.run",
            "console.runStmt",
            "console.format",
            "language.zh",
        ] {
            assert_ne!(t(key, Locale::En), key, "missing en {key}");
            assert_ne!(t(key, Locale::Zh), key, "missing zh {key}");
        }
        assert_eq!(t("language.zh", Locale::En), t("language.zh", Locale::Zh));
    }

    /// `en.json` and `zh.json` must expose exactly the same key set so neither
    /// locale silently drops a string the other renders.
    #[test]
    fn catalogues_share_key_set() {
        let mut en_keys: Vec<&String> = EN.keys().collect();
        en_keys.sort();
        let mut zh_keys: Vec<&String> = ZH.keys().collect();
        zh_keys.sort();
        assert_eq!(en_keys, zh_keys, "en/zh catalogue keys diverged");
    }

    /// Technical abbreviations that legitimately stay in Latin script inside the
    /// Chinese UI (CPU, JSON, EDGE, …). Every other `zh.json` value must carry
    /// at least one CJK glyph, otherwise the entry is an untranslated stub.
    const ACCEPTABLE_LATIN: &[&str] = &[
        "csv", "cpu%>", "index", "cpu", "en", "unique", "tag", "qps", "edge", "er", "json",
    ];

    fn has_cjk(s: &str) -> bool {
        s.chars().any(|c| {
            let u = c as u32;
            (0x3400..=0x4DBF).contains(&u)
                || (0x4E00..=0x9FFF).contains(&u)
                || (0xF900..=0xFAFF).contains(&u)
        })
    }

    #[test]
    fn chinese_entries_are_translated() {
        for (key, value) in ZH.iter() {
            if has_cjk(value) {
                continue;
            }
            let normalized: String = value
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            assert!(
                ACCEPTABLE_LATIN.contains(&normalized.as_str()),
                "zh entry {key} has no Chinese and is not an allowed abbreviation: {value}"
            );
        }
    }

    /// A `{}` in the English template needs a matching `{}` in the Chinese one,
    /// otherwise arguments shift and a slot renders raw braces.
    #[test]
    fn templates_share_placeholder_count() {
        for key in EN.keys() {
            let en_count = EN[key].matches("{}").count();
            let zh_count = ZH.get(key).map(|v| v.matches("{}").count()).unwrap_or(0);
            assert_eq!(
                en_count, zh_count,
                "placeholder count mismatch for {key}: en={en_count} zh={zh_count}"
            );
        }
    }

    /// Keys passed to `t`/`tf`/`tf_args` at call sites must exist in the
    /// catalogue, so a typo surfaces as a test failure instead of a raw key in
    /// the UI. Intentionally-absent keys from negative tests are skipped.
    #[test]
    fn call_site_keys_exist_in_catalogue() {
        const IGNORED: &[&str] = &["definitely.missing.key"];
        let root = env!("CARGO_MANIFEST_DIR");
        let src = std::path::Path::new(root).join("src");
        let mut seen = Vec::new();
        collect_call_site_keys(&src, &mut seen);
        seen.sort();
        seen.dedup();
        for key in seen {
            if IGNORED.contains(&key.as_str()) {
                continue;
            }
            // Only namespaced catalogue keys carry a dot; bare `t("...")` string
            // literals elsewhere in source are not catalogue lookups.
            if !key.contains('.') {
                continue;
            }
            // Catalogue keys are dot-separated identifiers (letters, digits,
            // dots). Anything else — doc-comment snippets, literal function
            // names — is not a real lookup and is skipped.
            if !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
                || !key.chars().any(|c| c.is_ascii_alphabetic())
            {
                continue;
            }
            assert!(
                EN.contains_key(&key),
                "call site references catalogue key not present in en.json: {key}"
            );
        }
    }

    fn collect_call_site_keys(dir: &std::path::Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_call_site_keys(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    scan_source_keys(&content, out);
                }
            }
        }
    }

    /// Delegates to [`scan_source_calls`] and keeps only the keys, so the
    /// forward and reverse key checks share one tokenizer.
    fn scan_source_keys(content: &str, out: &mut Vec<String>) {
        let mut calls = Vec::new();
        scan_source_calls(content, &mut calls);
        for (key, _) in calls {
            out.push(key);
        }
    }

    /// Skip a string literal starting at `start` (normal `"..."` or raw
    /// `r"..."` / `r#"..."#`). Returns the index just past the closing quote so
    /// the caller never scans catalogue-like text living inside a string.
    fn skip_string(bytes: &[u8], start: usize) -> usize {
        if bytes[start] == b'r' {
            let mut j = start + 1;
            let mut hashes = 0usize;
            while j < bytes.len() && bytes[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'"' {
                j += 1;
                while j < bytes.len() {
                    if bytes[j] == b'"' {
                        let mut k = j + 1;
                        let mut h = 0usize;
                        while k < bytes.len() && h < hashes && bytes[k] == b'#' {
                            k += 1;
                            h += 1;
                        }
                        if h == hashes {
                            return k;
                        }
                    }
                    j += 1;
                }
                return bytes.len();
            }
            return start + 1;
        }
        let mut j = start + 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' => j += 2,
                b'"' => return j + 1,
                _ => j += 1,
            }
        }
        bytes.len()
    }

    /// Parse a `t`/`tf`/`tf_args` call whose opening `(` is at `start`. Returns
    /// the key and the number of value arguments (0 for `t`, 1 for `tf`, the
    /// inline `&[...]` length for `tf_args`). Returns `None` when the call does
    /// not match the expected shape (e.g. a dynamically built key).
    fn parse_call(bytes: &[u8], start: usize) -> Option<(String, usize)> {
        let is_tf_args = bytes[start + 1] == b'f' && start + 2 < bytes.len() && bytes[start + 2] == b'_';
        let is_tf = !is_tf_args && bytes[start + 1] == b'f';
        let mut j = start;
        while j < bytes.len() && bytes[j] != b'(' {
            j += 1;
        }
        if j >= bytes.len() {
            return None;
        }
        j += 1; // past '('
        while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b'"' {
            return None;
        }
        j += 1;
        let ks = j;
        while j < bytes.len() && bytes[j] != b'"' {
            if bytes[j] == b'\\' {
                j += 2;
            } else {
                j += 1;
            }
        }
        if j >= bytes.len() {
            return None;
        }
        let key = String::from_utf8_lossy(&bytes[ks..j]).into_owned();
        j += 1; // past closing quote
        while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b',' {
            return None;
        }
        j += 1; // past comma after key
        while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
            j += 1;
        }
        // Skip the locale argument until the next top-level ',' or ')'.
        let mut depth = 0i32;
        while j < bytes.len() {
            match bytes[j] {
                b'(' | b'<' | b'[' | b'{' => depth += 1,
                b')' | b'>' | b']' | b'}' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                b',' if depth == 0 => break,
                _ => {}
            }
            j += 1;
        }
        let arg_count = if j < bytes.len() && bytes[j] == b')' {
            0 // t(key, locale): locale is the last argument
        } else {
            j += 1; // past the comma separating locale from the value args
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                j += 1;
            }
            if is_tf {
                1
            } else if is_tf_args {
                if j + 1 < bytes.len() && bytes[j] == b'&' && bytes[j + 1] == b'[' {
                    count_array_elems(bytes, j)?
                } else {
                    return None; // not an inline array literal
                }
            } else {
                // t(key, locale): the comma was the separator before the closing
                // `)`, with no value arguments following.
                0
            }
        };
        Some((key, arg_count))
    }

    /// Catalogue keys are dot-separated identifiers (letters, digits, dots) with
    /// at least one alphabetic character. Shared by the forward and reverse key
    /// scans so both agree on what counts as a real lookup.
    fn is_catalogue_key(s: &str) -> bool {
        !s.is_empty()
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
            && s.chars().any(|c| c.is_ascii_alphabetic())
    }

    /// Count top-level elements of an inline array literal beginning at `&[` in
    /// `b`. Returns `None` when the slice is not an inline `&[...]` literal
    /// (e.g. a variable reference) or is malformed, so callers can skip rather
    /// than guess a count.
    fn count_array_elems(b: &[u8], start: usize) -> Option<usize> {
        let mut j = start;
        while j < b.len() && b[j] != b'[' {
            j += 1;
        }
        if j >= b.len() {
            return None;
        }
        j += 1; // past '['
        let mut count = 0usize;
        let mut elem_started = false;
        let mut depth = 0i32;
        let mut k = j;
        while k < b.len() {
            match b[k] {
                b'[' => {
                    depth += 1;
                    elem_started = true;
                }
                b']' => {
                    if depth == 0 {
                        if elem_started {
                            count += 1;
                        }
                        return Some(count);
                    }
                    depth -= 1;
                }
                b',' => {
                    if depth == 0 {
                        count += 1;
                        elem_started = false;
                    }
                }
                b'"' | b'\'' => {
                    let q = b[k];
                    k += 1;
                    while k < b.len() && b[k] != q {
                        if b[k] == b'\\' {
                            k += 1;
                        }
                        k += 1;
                    }
                    elem_started = true;
                    continue;
                }
                b' ' | b'\t' | b'\n' | b'\r' => {}
                _ => {
                    elem_started = true;
                }
            }
            k += 1;
        }
        None
    }

    /// Walk `src/` and collect `(key, arg_count)` pairs from `t`/`tf`/`tf_args`
    /// call sites. `arg_count` is the number of value arguments the call passes
    /// (0 for `t`, 1 for `tf`, the inline `&[...]` length for `tf_args`); calls
    /// whose arguments are not inline literals are skipped.
    fn collect_call_sites(dir: &std::path::Path, out: &mut Vec<(String, usize)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_call_sites(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                // Skip this file: its own tests deliberately under-fill some
                // templates to document fallback behavior.
                if path.file_name().map(|n| n == "i18n.rs").unwrap_or(false) {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    scan_source_calls(&content, out);
                }
            }
        }
    }

    /// Walk `content` and collect `(key, arg_count)` pairs from `t`/`tf`/`tf_args`
    /// call sites. Comments and string literals are skipped first, so a `t("`
    /// appearing inside a doc comment or a SQL/JSON string cannot masquerade as
    /// a lookup or make the scanner jump past real call sites.
    fn scan_source_calls(content: &str, out: &mut Vec<(String, usize)>) {
        let bytes = content.as_bytes();
        let n = bytes.len();
        let mut i = 0;
        while i < n {
            let c = bytes[i];
            // Line comment (`//`, `///`, `//!`).
            if c == b'/' && i + 1 < n && bytes[i + 1] == b'/' {
                i += 2;
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            // Block comment.
            if c == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            // String literal (normal or raw). Never scan inside it.
            if c == b'"' || (c == b'r' && i + 1 < n && (bytes[i + 1] == b'"' || bytes[i + 1] == b'#')) {
                i = skip_string(bytes, i);
                continue;
            }
            // A `t`/`tf`/`tf_args` call that is not part of a larger identifier.
            if c == b't' {
                let pat_len = if i + 7 < n
                    && bytes[i + 1] == b'f'
                    && bytes[i + 2] == b'_'
                    && bytes[i + 3] == b'a'
                    && bytes[i + 4] == b'r'
                    && bytes[i + 5] == b'g'
                    && bytes[i + 6] == b's'
                    && bytes[i + 7] == b'('
                {
                    8
                } else if i + 2 < n && bytes[i + 1] == b'f' && bytes[i + 2] == b'(' {
                    3
                } else if i + 1 < n && bytes[i + 1] == b'(' {
                    2
                } else {
                    0
                };
                if pat_len > 0 {
                    let prev_ok = i == 0
                        || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
                    if prev_ok {
                        if let Some((key, arg_count)) = parse_call(bytes, i) {
                            out.push((key, arg_count));
                        }
                        i += pat_len;
                        continue;
                    }
                }
            }
            i += 1;
        }
    }

    /// Read a string literal starting at `start` (normal `"..."` or raw
    /// `r"..."` / `r#"..."#`) and return its inner text plus the index just past
    /// the closing delimiter. Returns `None` for malformed input.
    fn read_string(bytes: &[u8], start: usize) -> Option<(String, usize)> {
        if bytes[start] == b'r' {
            let mut j = start + 1;
            let mut hashes = 0usize;
            while j < bytes.len() && bytes[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'"' {
                j += 1;
                let ks = j;
                while j < bytes.len() {
                    if bytes[j] == b'"' {
                        let mut k = j + 1;
                        let mut h = 0usize;
                        while k < bytes.len() && h < hashes && bytes[k] == b'#' {
                            k += 1;
                            h += 1;
                        }
                        if h == hashes {
                            return Some((
                                String::from_utf8_lossy(&bytes[ks..j]).into_owned(),
                                k,
                            ));
                        }
                    }
                    j += 1;
                }
                return None;
            }
            return None;
        }
        let mut j = start + 1;
        let ks = j;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' => j += 2,
                b'"' => {
                    return Some((String::from_utf8_lossy(&bytes[ks..j]).into_owned(), j + 1))
                }
                _ => j += 1,
            }
        }
        None
    }

    /// Collect every string literal in `content` whose shape matches a catalogue
    /// key (`is_catalogue_key`). Keys reach the UI through many helpers
    /// (`label`, `button`, `path_button`, `deny`, match arms, …) that forward to
    /// `t` internally, so a `t`/`tf`/`tf_args`-only scan would miss them and
    /// falsely report live keys as dead. Comment and string contents are skipped.
    fn scan_catalogue_literals(content: &str, out: &mut Vec<String>) {
        let bytes = content.as_bytes();
        let n = bytes.len();
        let mut i = 0;
        while i < n {
            let c = bytes[i];
            if c == b'/' && i + 1 < n && bytes[i + 1] == b'/' {
                i += 2;
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if c == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            if c == b'"' || (c == b'r' && i + 1 < n && (bytes[i + 1] == b'"' || bytes[i + 1] == b'#')) {
                if let Some((text, end)) = read_string(bytes, i) {
                    if is_catalogue_key(&text) {
                        out.push(text);
                    }
                    i = end;
                } else {
                    i += 1;
                }
                continue;
            }
            i += 1;
        }
    }

    /// Recursively collect catalogue-shaped string literals under `dir`.
    fn collect_catalogue_literals(dir: &std::path::Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_catalogue_literals(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    scan_catalogue_literals(&content, out);
                }
            }
        }
    }

    /// English strings must stay ASCII (no CJK), so a mis-paste of Chinese into
    /// the English catalogue surfaces instead of shipping a half-translated UI.
    /// `language.zh` is the deliberate Chinese label shown for the zh option in
    /// the English UI, so it is allowed.
    const EN_CJK_ALLOWED: &[&str] = &["language.zh"];

    #[test]
    fn english_entries_have_no_cjk() {
        for (key, value) in EN.iter() {
            if EN_CJK_ALLOWED.contains(&key.as_str()) {
                continue;
            }
            assert!(
                !has_cjk(value),
                "en entry {key} contains CJK glyphs (likely a mis-paste): {value}"
            );
        }
    }

    /// Catalogue keys are only useful when a view renders them. A key no call
    /// site references is dead weight left by a renamed or dropped feature.
    /// Keys intentionally reserved (not yet wired to a view) live in
    /// `IGNORED_KEYS`.
    #[test]
    fn catalogue_keys_are_referenced() {
        const IGNORED_KEYS: &[&str] = &[];
        let root = env!("CARGO_MANIFEST_DIR");
        let src = std::path::Path::new(root).join("src");
        let mut seen = Vec::new();
        collect_catalogue_literals(&src, &mut seen);
        seen.sort();
        seen.dedup();
        let used: std::collections::HashSet<&str> = seen.iter().map(|s| s.as_str()).collect();
        let mut missing: Vec<&str> = EN
            .keys()
            .filter(|k| !IGNORED_KEYS.contains(&k.as_str()) && !used.contains(k.as_str()))
            .map(|k| k.as_str())
            .collect();
        missing.sort();
        assert!(
            missing.is_empty(),
            "catalogue keys never referenced at a call site: {missing:?}"
        );
    }

    /// The number of value arguments a call site passes to `tf`/`tf_args` must
    /// match the `{}` placeholder count of the template, otherwise a slot
    /// renders as raw braces or an argument is silently dropped. Keys built
    /// dynamically (variables, `format!`) are not statically scannable and are
    /// skipped.
    #[test]
    fn call_site_arg_count_matches_template() {
        let root = env!("CARGO_MANIFEST_DIR");
        let src = std::path::Path::new(root).join("src");
        let mut calls = Vec::new();
        collect_call_sites(&src, &mut calls);
        let mut mismatches: Vec<(String, usize, usize)> = Vec::new();
        for (key, arg_count) in calls {
            if !key.contains('.') || !is_catalogue_key(&key) {
                continue;
            }
            let Some(template) = EN.get(&key) else {
                continue; // covered by call_site_keys_exist_in_catalogue
            };
            let placeholders = template.matches("{}").count();
            if arg_count != placeholders {
                mismatches.push((key.clone(), arg_count, placeholders));
            }
        }
        assert!(
            mismatches.is_empty(),
            "arg count mismatches (key, passed, template_needs): {mismatches:?}"
        );
    }
}
