//! Interface message catalogue for the desktop client.
//!
//! The JSON files in `messages/` are the only place holding non-English
//! text, so Rust sources stay English-only. Lookups fall back to English
//! and then to the key itself, so a missing entry never blanks the UI.
//!
//! Stage one migrates the login view, the main window chrome (sidebar
//! groups, status bar, footer) and the console toolbar plus its empty-state
//! notices. History, favorites, parameters, cursor pager, export, inspect
//! and graph-send surfaces follow in later batches by adding keys here.

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
pub fn tf(key: &str, locale: Locale, arg: impl std::fmt::Display) -> String {
    t(key, locale).replacen("{}", &arg.to_string(), 1)
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
