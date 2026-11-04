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
}
