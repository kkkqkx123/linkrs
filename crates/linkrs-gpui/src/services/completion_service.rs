//! Console completion: keywords, functions, schema names and parameters.
//!
//! The suggestion sources mirror the web console: the lexical inventory from
//! `linkrs-grammar` (no parser needed), the current space schema snapshot
//! shared with the console panel, plus buffer-derived alias and parameter
//! maps. Everything here is synchronous; schema fetching stays with the
//! owning view, which refreshes the shared snapshot on space switches and
//! after schema-changing statements.

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::LazyLock};

use gpui_kit::component::input::{CompletionProvider, Rope, RopeExt};
use gpui_kit::{App, Task, Window};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    TextEdit,
};

/// Parsed view of `linkrs-grammar`'s committed keyword inventory.
#[derive(Debug)]
pub struct Inventory {
    /// Hard keywords in canonical (upper-case) spelling.
    pub keywords: Vec<String>,
    /// Upper-case names that force backtick quoting when used as identifiers.
    reserved: Vec<String>,
    /// Built-in function signatures.
    pub functions: Vec<FunctionSignature>,
}

/// A built-in function from the inventory.
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    /// Canonical (upper-case) name.
    pub name: String,
    /// Human-readable signature fragment, e.g. `aggregate (1)`.
    pub detail: String,
}

#[derive(Debug, serde::Deserialize)]
struct RawInventory {
    keywords: Vec<RawKeyword>,
    #[serde(default)]
    soft_keywords: Vec<String>,
    #[serde(default)]
    functions: Vec<RawFunction>,
}

#[derive(Debug, serde::Deserialize)]
struct RawKeyword {
    spelling: String,
}

#[derive(Debug, serde::Deserialize)]
struct RawFunction {
    name: String,
    #[serde(default)]
    arity: Option<u32>,
    #[serde(default)]
    variadic: bool,
    #[serde(default)]
    category: Option<String>,
}

impl Inventory {
    /// Parse the committed inventory. Pent-up parse failures fall back to an
    /// empty inventory so completion degrades instead of breaking the editor.
    pub fn load() -> Self {
        let fallback = Self {
            keywords: Vec::new(),
            reserved: Vec::new(),
            functions: Vec::new(),
        };
        let raw: RawInventory =
            serde_json::from_str(linkrs_grammar::inventory_json()).unwrap_or(RawInventory {
                keywords: Vec::new(),
                soft_keywords: Vec::new(),
                functions: Vec::new(),
            });
        let keywords: Vec<String> = raw.keywords.into_iter().map(|k| k.spelling).collect();
        let functions: Vec<FunctionSignature> = raw
            .functions
            .into_iter()
            .map(|f| {
                let category = f.category.unwrap_or_default();
                let detail = match (f.variadic, f.arity) {
                    (true, _) => format!("{category} (variadic)"),
                    (false, Some(arity)) => format!("{category} ({arity})"),
                    (false, None) => category,
                };
                FunctionSignature {
                    name: f.name,
                    detail,
                }
            })
            .collect();
        let mut reserved: Vec<String> = keywords.clone();
        reserved.extend(raw.soft_keywords);
        reserved.extend(functions.iter().map(|f| f.name.clone()));
        if keywords.is_empty() && functions.is_empty() {
            return fallback;
        }
        Self {
            keywords,
            reserved,
            functions,
        }
    }

    /// Process-wide inventory shared by every console editor.
    pub fn shared() -> &'static Self {
        static INVENTORY: LazyLock<Inventory> = LazyLock::new(Inventory::load);
        &INVENTORY
    }
}

/// Schema names of one space, feeding entity and property suggestions.
/// The owning view remembers which space the snapshot belongs to.
#[derive(Debug, Default, Clone)]
pub struct SchemaSnapshot {
    /// `(tag name, [(field name, data type)])`.
    pub tags: Vec<(String, Vec<(String, String)>)>,
    /// `(edge type name, [(field name, data type)])`.
    pub edges: Vec<(String, Vec<(String, String)>)>,
}

impl SchemaSnapshot {
    /// Project wire schema rows into suggestion shape.
    pub fn from_wire(
        tags: &[linkrs_wire::schema::TagInfo],
        edges: &[linkrs_wire::schema::EdgeTypeInfo],
    ) -> Self {
        Self {
            tags: tags
                .iter()
                .map(|tag| {
                    (
                        tag.name.clone(),
                        tag.fields
                            .iter()
                            .map(|f| (f.name.clone(), f.data_type.clone()))
                            .collect(),
                    )
                })
                .collect(),
            edges: edges
                .iter()
                .map(|edge| {
                    (
                        edge.name.clone(),
                        edge.fields
                            .iter()
                            .map(|f| (f.name.clone(), f.data_type.clone()))
                            .collect(),
                    )
                })
                .collect(),
        }
    }

    /// Look up an entity (tag first, then edge type) case-insensitively.
    fn entity_fields(&self, name: &str) -> Option<(&str, &[(String, String)])> {
        let key = name.to_lowercase();
        for (entity, fields) in &self.tags {
            if entity.to_lowercase() == key {
                return Some((entity, fields));
            }
        }
        for (entity, fields) in &self.edges {
            if entity.to_lowercase() == key {
                return Some((entity, fields));
            }
        }
        None
    }
}

/// Snapshot handle shared between the console panel (writer) and the
/// completion provider (reader).
#[derive(Debug, Default, Clone)]
pub struct SharedSchemaSnapshot(Rc<RefCell<Option<SchemaSnapshot>>>);

impl SharedSchemaSnapshot {
    /// Empty handle; the provider degrades to inventory-only suggestions
    /// until the first successful refresh.
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a fresh snapshot, replacing whatever the space held before.
    pub fn store(&self, snapshot: SchemaSnapshot) {
        *self.0.borrow_mut() = Some(snapshot);
    }
}

/// One computed suggestion before LSP mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Suggestion {
    /// Displayed label, already quoted when required.
    label: String,
    /// Replacement text for the current word range.
    insert: String,
    /// Ordering group mirroring the web console (keyword, tag, edge,
    /// field, function).
    group: char,
    /// LSP kind for the menu icon.
    kind: CompletionItemKind,
    /// Detail line shown next to the label.
    detail: String,
}

/// Byte range and text of the word under the caret. Word characters are
/// ASCII alphanumerics plus `_`, `$` and `@` so parameter prefixes stay
/// attached to their sigil.
fn word_at(text: &str, offset: usize) -> (usize, &str) {
    let offset = offset.min(text.len());
    let bytes = text.as_bytes();
    let mut start = offset;
    while start > 0 {
        let byte = bytes[start - 1];
        if !(byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte == b'@') {
            break;
        }
        start -= 1;
    }
    (start, &text[start..offset])
}

/// Identifier immediately before a `.` ending at `dot`, e.g. the `n` in
/// `n.|`. Returns `None` when no identifier precedes the dot.
fn owner_before_dot(text: &str, dot: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut end = dot;
    while end > 0 && bytes[end - 1] == b' ' {
        end -= 1;
    }
    let mut start = end;
    while start > 0 {
        let byte = bytes[start - 1];
        if !(byte.is_ascii_alphanumeric() || byte == b'_') {
            break;
        }
        start -= 1;
    }
    if start < end
        && (bytes[start].is_ascii_alphabetic() || bytes[start] == b'_')
        && text.is_char_boundary(start)
    {
        Some(&text[start..end])
    } else {
        None
    }
}

/// Map query variable aliases to the entity they bind, covering the common
/// inline `(alias : Entity)` and `[alias : Entity]` forms.
fn build_alias_map(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let open = bytes[index];
        if open != b'(' && open != b'[' {
            index += 1;
            continue;
        }
        let mut cursor = index + 1;
        let alias = read_ident(bytes, &mut cursor);
        skip_spaces(bytes, &mut cursor);
        let Some(alias) = alias else {
            index += 1;
            continue;
        };
        if cursor >= bytes.len() || bytes[cursor] != b':' {
            index += 1;
            continue;
        }
        cursor += 1;
        skip_spaces(bytes, &mut cursor);
        if let Some(entity) = read_ident(bytes, &mut cursor) {
            map.insert(alias.to_lowercase(), entity.to_lowercase());
            index = cursor;
        } else {
            index += 1;
        }
    }
    map
}

/// Read a plain identifier at `cursor`, advancing past it.
fn read_ident<'a>(bytes: &'a [u8], cursor: &mut usize) -> Option<&'a str> {
    let start = *cursor;
    if start < bytes.len() && (bytes[start].is_ascii_alphabetic() || bytes[start] == b'_') {
        *cursor += 1;
        while *cursor < bytes.len()
            && (bytes[*cursor].is_ascii_alphanumeric() || bytes[*cursor] == b'_')
        {
            *cursor += 1;
        }
        std::str::from_utf8(&bytes[start..*cursor]).ok()
    } else {
        None
    }
}

/// Advance past ASCII whitespace.
fn skip_spaces(bytes: &[u8], cursor: &mut usize) {
    while *cursor < bytes.len() && bytes[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
}

/// Distinct `$` or `@` parameter names in buffer order.
fn collect_params(text: &str, sigil: u8) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == sigil {
            let mut cursor = index + 1;
            if let Some(name) = read_ident(bytes, &mut cursor) {
                let name = name.to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
                index = cursor;
                continue;
            }
        }
        index += 1;
    }
    out
}

/// Wrap a schema name in backticks unless it is a plain identifier that does
/// not collide with a reserved word.
fn quote_if_needed(name: &str, reserved_upper: &[String]) -> String {
    let mut bytes = name.bytes();
    let plain = matches!(bytes.next(), Some(b) if b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if plain && !reserved_upper.iter().any(|r| r == &name.to_uppercase()) {
        name.to_string()
    } else {
        format!("`{}`", name.replace('`', ""))
    }
}

/// Compute suggestions for the caret, mirroring the web console providers.
fn suggest(
    text: &str,
    offset: usize,
    inventory: &Inventory,
    snapshot: Option<&SchemaSnapshot>,
) -> Vec<Suggestion> {
    let offset = offset.min(text.len());
    let (word_start, word) = word_at(text, offset);
    if word.is_empty() {
        let trigger = text.as_bytes().get(word_start.saturating_sub(1)).copied();
        return match trigger {
            Some(b'.') => {
                let owner = owner_before_dot(text, word_start - 1);
                property_suggestions(text, owner, "", inventory, snapshot)
            }
            Some(b':') => entity_suggestions("", inventory, snapshot),
            Some(b'$') => param_suggestions(text, b'$', ""),
            Some(b'@') => param_suggestions(text, b'@', ""),
            _ => Vec::new(),
        };
    }
    if let Some(prefix) = word.strip_prefix('$') {
        return param_suggestions(text, b'$', prefix);
    }
    if let Some(prefix) = word.strip_prefix('@') {
        return param_suggestions(text, b'@', prefix);
    }
    if word_start > 0 && text.as_bytes()[word_start - 1] == b'.' {
        let owner = owner_before_dot(text, word_start - 1);
        return property_suggestions(text, owner, word, inventory, snapshot);
    }
    let mut out = keyword_suggestions(word, inventory);
    out.extend(entity_suggestions(word, inventory, snapshot));
    out.extend(function_suggestions(word, inventory));
    sort_suggestions(&mut out);
    out.truncate(60);
    out
}

/// Keyword suggestions for a general prefix.
fn keyword_suggestions(prefix: &str, inventory: &Inventory) -> Vec<Suggestion> {
    let upper = prefix.to_uppercase();
    inventory
        .keywords
        .iter()
        .filter(|keyword| keyword.starts_with(&upper))
        .map(|keyword| Suggestion {
            label: keyword.clone(),
            insert: keyword.clone(),
            group: '1',
            kind: CompletionItemKind::KEYWORD,
            detail: String::new(),
        })
        .collect()
}

/// Tag and edge type suggestions for a general or post-colon prefix.
fn entity_suggestions(
    prefix: &str,
    inventory: &Inventory,
    snapshot: Option<&SchemaSnapshot>,
) -> Vec<Suggestion> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    let lower = prefix.to_lowercase();
    let reserved_upper = inventory
        .reserved
        .iter()
        .map(|name| name.to_uppercase())
        .collect::<Vec<_>>();
    let mut out = Vec::new();
    for (name, _) in &snapshot.tags {
        if name.to_lowercase().starts_with(&lower) {
            out.push(Suggestion {
                label: name.clone(),
                insert: quote_if_needed(name, &reserved_upper),
                group: '2',
                kind: CompletionItemKind::CLASS,
                detail: "tag".to_string(),
            });
        }
    }
    for (name, _) in &snapshot.edges {
        if name.to_lowercase().starts_with(&lower) {
            out.push(Suggestion {
                label: name.clone(),
                insert: quote_if_needed(name, &reserved_upper),
                group: '3',
                kind: CompletionItemKind::INTERFACE,
                detail: "edge type".to_string(),
            });
        }
    }
    out
}

/// Function suggestions for a general prefix.
fn function_suggestions(prefix: &str, inventory: &Inventory) -> Vec<Suggestion> {
    let upper = prefix.to_uppercase();
    inventory
        .functions
        .iter()
        .filter(|func| func.name.to_uppercase().starts_with(&upper))
        .map(|func| Suggestion {
            label: func.name.clone(),
            insert: format!("{}()", func.name),
            group: '5',
            kind: CompletionItemKind::FUNCTION,
            detail: func.detail.clone(),
        })
        .collect()
}

/// Property suggestions scoped to the entity before the dot (resolved
/// through the alias map when the owner is a query variable).
fn property_suggestions(
    text: &str,
    owner: Option<&str>,
    prefix: &str,
    _inventory: &Inventory,
    snapshot: Option<&SchemaSnapshot>,
) -> Vec<Suggestion> {
    let (Some(owner), Some(snapshot)) = (owner, snapshot) else {
        return Vec::new();
    };
    let aliases = build_alias_map(text);
    let key = owner.to_lowercase();
    let entity = aliases.get(&key).map(String::as_str).unwrap_or(&key);
    let lower = prefix.to_lowercase();
    let mut out = Vec::new();
    if let Some((entity_name, fields)) = snapshot.entity_fields(entity) {
        for (field, data_type) in fields {
            if field.to_lowercase().starts_with(&lower) {
                out.push(Suggestion {
                    label: field.clone(),
                    insert: field.clone(),
                    group: '4',
                    kind: CompletionItemKind::FIELD,
                    detail: format!("{entity_name}.{field} ({data_type})"),
                });
            }
        }
    }
    sort_suggestions(&mut out);
    out.truncate(60);
    out
}

/// Parameter suggestions for a sigil prefix.
fn param_suggestions(text: &str, sigil: u8, prefix: &str) -> Vec<Suggestion> {
    let lower = prefix.to_lowercase();
    let mut out: Vec<Suggestion> = collect_params(text, sigil)
        .into_iter()
        .filter(|name| name.to_lowercase().starts_with(&lower))
        .map(|name| Suggestion {
            insert: format!("{}{}", sigil as char, name),
            label: name,
            group: '1',
            kind: CompletionItemKind::VARIABLE,
            detail: "parameter".to_string(),
        })
        .collect();
    sort_suggestions(&mut out);
    out.truncate(60);
    out
}

/// Order by group, then case-insensitively by label.
fn sort_suggestions(suggestions: &mut [Suggestion]) {
    suggestions
        .sort_by(|a, b| (a.group, a.label.to_lowercase()).cmp(&(b.group, b.label.to_lowercase())));
}

/// Map a computed suggestion onto the menu item, replacing the word under
/// the caret. An explicit text edit is required: with only `insert_text`
/// the menu would append after the typed prefix instead of replacing it.
fn to_item(suggestion: &Suggestion, start: usize, end: usize, text: &Rope) -> CompletionItem {
    let range = lsp_types::Range {
        start: text.offset_to_position(start),
        end: text.offset_to_position(end),
    };
    CompletionItem {
        label: suggestion.label.clone(),
        kind: Some(suggestion.kind.clone()),
        detail: if suggestion.detail.is_empty() {
            None
        } else {
            Some(suggestion.detail.clone())
        },
        sort_text: Some(format!(
            "{}{}",
            suggestion.group,
            suggestion.label.to_lowercase()
        )),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: suggestion.insert.clone(),
        })),
        ..Default::default()
    }
}

/// Console completion provider: inventory plus the shared schema snapshot.
#[derive(Clone)]
pub struct LinkrsCompletionProvider {
    inventory: &'static Inventory,
    schema: SharedSchemaSnapshot,
}

impl LinkrsCompletionProvider {
    /// Build a provider reading the given snapshot handle. The handle is
    /// shared with the owning view, which refreshes it as the space and
    /// schema evolve.
    pub fn new(schema: SharedSchemaSnapshot) -> Self {
        Self {
            inventory: Inventory::shared(),
            schema,
        }
    }
}

impl CompletionProvider for LinkrsCompletionProvider {
    /// Every keystroke re-asks the provider; the suggestion pass itself
    /// decides whether the caret sits in a completable position and
    /// answers with an empty list to dismiss the menu otherwise.
    fn is_completion_trigger(&self, _offset: usize, _new_text: &str, _cx: &mut App) -> bool {
        true
    }

    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let script = text.to_string();
        let snapshot = self.schema.0.borrow().clone();
        let (word_start, _) = word_at(&script, offset.min(script.len()));
        let suggestions = suggest(&script, offset, self.inventory, snapshot.as_ref());
        let items = suggestions
            .iter()
            .map(|suggestion| to_item(suggestion, word_start, offset.min(script.len()), text))
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory() -> Inventory {
        Inventory::load()
    }

    fn snapshot() -> SchemaSnapshot {
        SchemaSnapshot {
            tags: vec![(
                "Person".to_string(),
                vec![
                    ("name".to_string(), "string".to_string()),
                    ("age".to_string(), "int".to_string()),
                ],
            )],
            edges: vec![(
                "Knows".to_string(),
                vec![("since".to_string(), "int".to_string())],
            )],
        }
    }

    #[test]
    fn inventory_parses_committed_keywords_and_functions() {
        let inventory = inventory();
        assert!(inventory.keywords.len() > 200);
        assert!(inventory.keywords.contains(&"MATCH".to_string()));
        assert!(inventory.functions.len() > 200);
        assert!(inventory.functions.iter().any(|f| f.name == "AVG"));
    }

    #[test]
    fn general_prefix_mixes_keywords_entities_and_functions() {
        let inventory = inventory();
        let snap = snapshot();
        let items = suggest("MAT", 3, &inventory, Some(&snap));
        assert!(items.iter().any(|s| s.label == "MATCH" && s.group == '1'));
        let items = suggest("Pers", 4, &inventory, Some(&snap));
        assert!(items.iter().any(|s| s.label == "Person" && s.group == '2'));
        let items = suggest("AV", 2, &inventory, Some(&snap));
        assert!(items.iter().any(|s| s.label == "AVG" && s.group == '5'));
    }

    #[test]
    fn dot_prefix_scopes_properties_to_owner_entity() {
        let inventory = inventory();
        let snap = snapshot();
        let items = suggest("MATCH (n:Person) RETURN n.", 26, &inventory, Some(&snap));
        let labels: Vec<&str> = items.iter().map(|s| s.label.as_str()).collect();
        assert!(labels.contains(&"name"));
        assert!(labels.contains(&"age"));
        assert!(!labels.contains(&"since"));
    }

    #[test]
    fn property_prefix_filters_within_owner() {
        let inventory = inventory();
        let snap = snapshot();
        let items = suggest("MATCH (n:Person) RETURN n.na", 28, &inventory, Some(&snap));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "name");
        assert_eq!(items[0].detail, "Person.name (string)");
    }

    #[test]
    fn alias_map_resolves_edge_variables() {
        let inventory = inventory();
        let snap = snapshot();
        let items = suggest(
            "MATCH ()-[e:Knows]->() RETURN e.",
            32,
            &inventory,
            Some(&snap),
        );
        assert!(items.iter().any(|s| s.label == "since"));
    }

    #[test]
    fn dollar_prefix_collects_buffer_params() {
        let inventory = inventory();
        let snap = snapshot();
        let items = suggest(
            "MATCH (n) WHERE n.age > $ag RETURN $a",
            38,
            &inventory,
            Some(&snap),
        );
        assert!(items
            .iter()
            .any(|s| s.label == "ag" && s.kind == CompletionItemKind::VARIABLE));
    }

    #[test]
    fn empty_prefix_after_dot_lists_all_owner_properties() {
        let inventory = inventory();
        let snap = snapshot();
        let items = suggest("RETURN Knows.", 13, &inventory, Some(&snap));
        assert!(items.iter().any(|s| s.label == "since"));
    }

    #[test]
    fn breaker_positions_yield_no_suggestions() {
        let inventory = inventory();
        let snap = snapshot();
        assert!(suggest("MATCH ", 6, &inventory, Some(&snap)).is_empty());
        assert!(suggest("", 0, &inventory, Some(&snap)).is_empty());
    }

    #[test]
    fn quoting_wraps_reserved_and_compound_names() {
        let inventory = inventory();
        assert_eq!(quote_if_needed("Person", &inventory.reserved), "Person");
        assert_eq!(quote_if_needed("MATCH", &inventory.reserved), "`MATCH`");
        assert_eq!(quote_if_needed("my tag", &inventory.reserved), "`my tag`");
    }
}
