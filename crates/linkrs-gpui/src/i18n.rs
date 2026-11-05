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

    fn scan_source_keys(content: &str, out: &mut Vec<String>) {
        let bytes = content.as_bytes();
        let patterns: &[&str] = &["tf_args(", "tf(", "t("];
        let mut i = 0;
        while i < bytes.len() {
            let mut found: Option<(usize, &str)> = None;
            for pat in patterns {
                if let Some(rel) = content[i..].find(pat) {
                    let abs = i + rel;
                    let prev_ok = abs == 0
                        || !(bytes[abs - 1].is_ascii_alphanumeric() || bytes[abs - 1] == b'_');
                    if prev_ok {
                        if let Some((best, _)) = found {
                            if abs < best {
                                found = Some((abs, pat));
                            }
                        } else {
                            found = Some((abs, pat));
                        }
                    }
                }
            }
            let Some((abs, pat)) = found else { break };
            let mut j = abs + pat.len();
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'"' {
                j += 1;
                let start = j;
                while j < bytes.len() && bytes[j] != b'"' {
                    if bytes[j] == b'\\' {
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                let key = &content[start..j];
                if !key.is_empty() {
                    out.push(key.to_string());
                }
                i = j + 1;
            } else {
                i = j;
            }
        }
    }
}
