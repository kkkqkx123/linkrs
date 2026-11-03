//! Linkrs query language registration for the embedded editor.
//!
//! The grammar comes from `linkrs-grammar`, the same crate the backend
//! parses with, statically linked into this binary. Registration only
//! publishes the syntax handle and the highlight query to gpui-kit's
//! language registry; all highlighting work stays inside the kit.

use gpui_kit::component::highlighter::{GrammarConfig, LanguageRegistry};

/// Publish the `linkrs` language to the editor stack. Safe to call once at
/// startup, before any console editor is created. A failed registration only
/// leaves the editor without tree-sitter highlighting; execution is
/// unaffected.
pub fn register_linkrs_language() {
    let config = GrammarConfig::new(
        "linkrs",
        linkrs_grammar::language(),
        Vec::new(),
        linkrs_grammar::highlights_query(),
        "",
        "",
    );
    LanguageRegistry::singleton().register("linkrs", &config);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linkrs_language_registers_with_a_grammar() {
        register_linkrs_language();
        let config = LanguageRegistry::singleton()
            .language("linkrs")
            .expect("linkrs is registered");
        assert!(config.has_grammar());
    }
}
