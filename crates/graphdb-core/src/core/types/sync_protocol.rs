use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! numeric_version_type {
    ($name:ident, $display_prefix:literal) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[repr(transparent)]
        pub struct $name(u64);

        impl $name {
            pub const ZERO: Self = Self(0);

            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, concat!($display_prefix, "{}"), self.0)
            }
        }
    };
}

numeric_version_type!(CommitLsn, "lsn:");
numeric_version_type!(IndexGeneration, "generation:");
numeric_version_type!(ManifestEpoch, "manifest:");
numeric_version_type!(LeaseEpoch, "lease:");

macro_rules! string_identifier_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, String> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(concat!(stringify!($name), " cannot be empty").to_string());
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

string_identifier_type!(TargetId);
string_identifier_type!(OrderingKey);
string_identifier_type!(IdempotencyKey);

#[cfg(test)]
mod tests {
    use super::{CommitLsn, IdempotencyKey, IndexGeneration, TargetId};

    #[test]
    fn version_domains_remain_distinct() {
        let lsn = CommitLsn::new(42);
        let generation = IndexGeneration::new(42);
        assert_eq!(lsn.get(), generation.get());
        assert_eq!(lsn.to_string(), "lsn:42");
        assert_eq!(generation.to_string(), "generation:42");
    }

    #[test]
    fn string_identifiers_reject_empty_values() {
        assert!(TargetId::new(" ").is_err());
        assert!(IdempotencyKey::new("event-1").is_ok());
    }
}
