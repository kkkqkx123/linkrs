//! Vertex Storage Module
//!
//! Provides columnar storage for vertex data with MVCC timestamp support.
//!
//! ## Components
//!
//! - `VertexTable`: Main vertex storage with columnar layout
//! - `IdIndexer`: External ID to internal ID mapping
//! - `ColumnStore`: Columnar property storage
//! - `VertexTimestamp`: MVCC timestamp tracking for vertices

pub mod column;
pub mod gc_manager;
pub mod id_indexer;
pub mod tiering;
pub mod vertex_table;
pub mod vertex_timestamp;
pub mod write_scope;

/// Debug-build latch-order assertions for the vertex table latches.
///
/// The in-table order is identity before column segments: a claim of a
/// lower rank may be held while acquiring a higher one, never the reverse.
/// Claims form a per-thread stack checked at acquisition time; release
/// builds compile the checks away.
pub(crate) mod latch_order {
    pub(crate) const RANK_IDENTITY: u8 = 1;
    pub(crate) const RANK_SEGMENT: u8 = 2;

    #[cfg(debug_assertions)]
    thread_local! {
        static HELD: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// RAII wrapper forwarding lock-guard access while claiming `rank`
    /// for the duration of the hold.
    pub(crate) struct Guard<G> {
        inner: G,
        #[cfg(debug_assertions)]
        rank: u8,
    }

    impl<G> Guard<G> {
        pub(crate) fn claim(inner: G, rank: u8, site: &'static str) -> Self {
            acquire(rank, site);
            Self {
                inner,
                #[cfg(debug_assertions)]
                rank,
            }
        }
    }

    impl<G: std::ops::Deref> std::ops::Deref for Guard<G> {
        type Target = G::Target;

        #[inline]
        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    impl<G: std::ops::DerefMut> std::ops::DerefMut for Guard<G> {
        #[inline]
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.inner
        }
    }

    impl<G> Drop for Guard<G> {
        #[inline]
        fn drop(&mut self) {
            #[cfg(debug_assertions)]
            release(self.rank);
        }
    }

    #[cfg(debug_assertions)]
    fn acquire(rank: u8, site: &'static str) {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(&top) = held.last() {
                assert!(
                    rank >= top,
                    "latch order violation: acquiring {site} (rank {rank}) while holding a higher-ranked latch (top rank {top})"
                );
            }
            held.push(rank);
        });
    }

    #[cfg(not(debug_assertions))]
    #[inline]
    fn acquire(_rank: u8, _site: &'static str) {}

    #[cfg(debug_assertions)]
    fn release(rank: u8) {
        HELD.with(|held| {
            let popped = held.borrow_mut().pop();
            debug_assert_eq!(
                popped,
                Some(rank),
                "latch claims must release in LIFO order"
            );
        });
    }

    #[cfg(not(debug_assertions))]
    #[inline]
    fn release(_rank: u8) {}
}

// Alias: external code uses `crate::vertex::column_store::*`.
pub use column as column_store;

use crate::types::StoragePropertyDef;

pub use column::ColumnStore;
pub use gc_manager::VertexGcConfig;
pub use id_indexer::{IdIndexer, IdKey, PkLookup};
pub use vertex_table::ShardedVertexTable;
pub use vertex_timestamp::VertexTimestamp;
pub use write_scope::{WriteScope, MAX_WRITE_SCOPE_KEYS};

use graphdb_core::{DataType, StorageError, StorageResult, Value};

pub use graphdb_core::types::{LabelId, Timestamp, VertexId, INVALID_TIMESTAMP, MAX_TIMESTAMP};

#[derive(Debug, Clone)]
pub struct VertexRecord {
    pub vid: VertexId,
    pub internal_id: u32,
    pub properties: Vec<(String, Value)>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VertexSchema {
    pub label_id: LabelId,
    pub label_name: String,
    pub properties: Vec<StoragePropertyDef>,
    pub primary_key_index: usize,
    pub schema_version: u64,
}

impl VertexSchema {
    /// Validate schema at creation time
    /// Ensures primary key exists and has a valid type for use as a key
    pub fn validate_on_creation(&self) -> Result<(), String> {
        // Validate primary key index
        if self.properties.is_empty() {
            return Err("Schema must have at least one property".to_string());
        }

        if self.primary_key_index >= self.properties.len() {
            return Err(format!(
                "Invalid primary key index: {} >= property count {}",
                self.primary_key_index,
                self.properties.len()
            ));
        }

        let primary_key_prop = &self.properties[self.primary_key_index];

        // Validate primary key type is suitable for keys (must be comparable and hashable)
        Self::validate_key_type(&primary_key_prop.data_type, &primary_key_prop.name)?;

        // Check if property name is valid (non-empty, valid identifier)
        if primary_key_prop.name.is_empty() {
            return Err("Primary key name cannot be empty".to_string());
        }

        Self::validate_identifier(&primary_key_prop.name)?;

        // Validate all property names are unique and valid
        let mut seen_names = std::collections::HashSet::new();
        for prop in &self.properties {
            if !seen_names.insert(&prop.name) {
                return Err(format!("Duplicate property name: '{}'", prop.name));
            }

            // Validate each property name
            if prop.name.is_empty() {
                return Err("Property name cannot be empty".to_string());
            }

            Self::validate_identifier(&prop.name)?;

            // Validate property data types are not Empty or Null
            Self::validate_property_type(&prop.data_type, &prop.name)?;
        }

        Ok(())
    }

    /// Validate that an identifier (name) follows valid rules
    /// Must start with letter or underscore, contain only alphanumeric + underscore
    fn validate_identifier(name: &str) -> Result<(), String> {
        let first_char = match name.chars().next() {
            Some(c) => c,
            None => return Err("Identifier cannot be empty".to_string()),
        };

        if !first_char.is_ascii_alphabetic() && first_char != '_' {
            return Err(format!(
                "Identifier '{}' must start with ASCII letter or underscore, got '{}'",
                name, first_char
            ));
        }

        // Check all characters are ASCII alphanumeric or underscore
        for (i, c) in name.chars().enumerate() {
            if !c.is_ascii_alphanumeric() && c != '_' {
                return Err(format!(
                    "Identifier '{}' contains invalid character '{}' at position {}. \
                     Only ASCII letters, digits, and underscores are allowed.",
                    name, c, i
                ));
            }
        }

        Ok(())
    }

    /// Validate that a data type is suitable for use as a primary key.
    ///
    /// The primary key column is a materialized mirror of the external vertex
    /// id, so only the id-compatible families are allowed: the integer family
    /// and strings. Everything else is rejected at schema creation time.
    fn validate_key_type(
        data_type: &graphdb_core::DataType,
        prop_name: &str,
    ) -> Result<(), String> {
        use graphdb_core::DataType;

        let allowed = matches!(
            data_type,
            DataType::SmallInt
                | DataType::Int
                | DataType::BigInt
                | DataType::String
                | DataType::FixedString(_)
        );
        if !allowed {
            return Err(format!(
                "Primary key '{}' has invalid type '{:?}'. \
                 The primary key column mirrors the vertex id, so only \
                 SmallInt, Int, BigInt, String and FixedString are allowed",
                prop_name, data_type
            ));
        }

        Ok(())
    }

    /// Validate that a property data type is allowed
    /// Rejects Empty and Null types which don't make sense as properties
    fn validate_property_type(
        data_type: &graphdb_core::DataType,
        prop_name: &str,
    ) -> Result<(), String> {
        use graphdb_core::DataType;

        match data_type {
            DataType::Empty => Err(format!(
                "Property '{}' cannot have type Empty - properties must have valid types",
                prop_name
            )),
            DataType::Null => Err(format!(
                "Property '{}' cannot have type Null - use nullable=true instead",
                prop_name
            )),
            _ => Ok(()),
        }
    }
}

/// Derive the primary key mirror value for an external id.
///
/// The primary key column is not an independent identity: it materializes the
/// external vertex id in the column's own type. Integer keys widen or render
/// into the column type; text keys are kept for string columns and must parse
/// for integer columns. Anything that cannot round-trip is an error.
pub(crate) fn primary_key_mirror_value(data_type: &DataType, key: &IdKey) -> StorageResult<Value> {
    let out_of_range = |detail: String| StorageError::invalid_input(detail);
    let int_mirror = |id: i64| -> StorageResult<Value> {
        match data_type {
            DataType::SmallInt => i16::try_from(id).map(Value::SmallInt).map_err(|_| {
                out_of_range(format!("Vertex id {} overflows SmallInt primary key", id))
            }),
            DataType::Int => i32::try_from(id)
                .map(Value::Int)
                .map_err(|_| out_of_range(format!("Vertex id {} overflows Int primary key", id))),
            DataType::BigInt => Ok(Value::BigInt(id)),
            DataType::String | DataType::FixedString(_) => Ok(Value::string(id.to_string())),
            _ => Err(out_of_range(format!(
                "Primary key type {:?} cannot mirror a vertex id",
                data_type
            ))),
        }
    };
    match key {
        IdKey::Int(id) => int_mirror(*id),
        IdKey::Text(text) => match data_type {
            DataType::String | DataType::FixedString(_) => Ok(Value::string(text.clone())),
            DataType::SmallInt | DataType::Int | DataType::BigInt => text
                .parse::<i64>()
                .map_err(|_| {
                    out_of_range(format!(
                        "Text vertex id {:?} cannot mirror integer primary key",
                        text
                    ))
                })
                .and_then(int_mirror),
            _ => Err(out_of_range(format!(
                "Primary key type {:?} cannot mirror a vertex id",
                data_type
            ))),
        },
    }
}
