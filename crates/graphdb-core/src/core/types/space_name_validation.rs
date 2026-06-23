//! Space Name Validation
//!
//! Provides unified validation for space names across the codebase.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceNameValidationError {
    Empty,
    TooLong(usize),
    InvalidStart(char),
    InvalidCharacter(char),
    ReservedName(String),
}

impl fmt::Display for SpaceNameValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "Space name cannot be empty"),
            Self::TooLong(len) => write!(f, "Space name too long: {} characters (max 64)", len),
            Self::InvalidStart(c) => {
                write!(f, "Space name must start with a letter, found: '{}'", c)
            }
            Self::InvalidCharacter(c) => {
                write!(f, "Space name contains invalid character: '{}'", c)
            }
            Self::ReservedName(name) => write!(f, "Space name '{}' is reserved", name),
        }
    }
}

impl std::error::Error for SpaceNameValidationError {}

const MAX_SPACE_NAME_LENGTH: usize = 64;
const INVALID_CHARS: &[char] = &[
    ' ', '\t', '\n', '\r', ',', ';', '(', ')', '[', ']', '{', '}', '.', '/', '\\', '\'', '"',
];
const RESERVED_NAMES: &[&str] = &[
    "system",
    "information_schema",
    "mysql",
    "performance_schema",
];

pub fn validate_space_name(name: &str) -> Result<(), SpaceNameValidationError> {
    if name.is_empty() {
        return Err(SpaceNameValidationError::Empty);
    }

    if name.len() > MAX_SPACE_NAME_LENGTH {
        return Err(SpaceNameValidationError::TooLong(name.len()));
    }

    let first_char = name.chars().next().unwrap();
    if first_char == '_' {
        return Err(SpaceNameValidationError::InvalidStart('_'));
    }

    if first_char.is_ascii_digit() {
        return Err(SpaceNameValidationError::InvalidStart(first_char));
    }

    if !first_char.is_alphabetic() {
        return Err(SpaceNameValidationError::InvalidStart(first_char));
    }

    for c in name.chars() {
        if INVALID_CHARS.contains(&c) {
            return Err(SpaceNameValidationError::InvalidCharacter(c));
        }
    }

    let lower_name = name.to_lowercase();
    for reserved in RESERVED_NAMES {
        if lower_name == *reserved {
            return Err(SpaceNameValidationError::ReservedName(name.to_string()));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_space_names() {
        assert!(validate_space_name("test").is_ok());
        assert!(validate_space_name("TestSpace").is_ok());
        assert!(validate_space_name("space123").is_ok());
        assert!(validate_space_name("my_space").is_ok());
        assert!(validate_space_name("a").is_ok());
    }

    #[test]
    fn test_empty_name() {
        assert_eq!(
            validate_space_name(""),
            Err(SpaceNameValidationError::Empty)
        );
    }

    #[test]
    fn test_too_long_name() {
        let long_name = "a".repeat(65);
        assert_eq!(
            validate_space_name(&long_name),
            Err(SpaceNameValidationError::TooLong(65))
        );

        let max_name = "a".repeat(64);
        assert!(validate_space_name(&max_name).is_ok());
    }

    #[test]
    fn test_invalid_start() {
        assert_eq!(
            validate_space_name("_test"),
            Err(SpaceNameValidationError::InvalidStart('_'))
        );
        assert_eq!(
            validate_space_name("123space"),
            Err(SpaceNameValidationError::InvalidStart('1'))
        );
        assert_eq!(
            validate_space_name("-test"),
            Err(SpaceNameValidationError::InvalidStart('-'))
        );
    }

    #[test]
    fn test_invalid_characters() {
        assert_eq!(
            validate_space_name("test space"),
            Err(SpaceNameValidationError::InvalidCharacter(' '))
        );
        assert_eq!(
            validate_space_name("test;drop"),
            Err(SpaceNameValidationError::InvalidCharacter(';'))
        );
        assert_eq!(
            validate_space_name("test.name"),
            Err(SpaceNameValidationError::InvalidCharacter('.'))
        );
    }

    #[test]
    fn test_reserved_names() {
        assert!(matches!(
            validate_space_name("system"),
            Err(SpaceNameValidationError::ReservedName(_))
        ));
        assert!(matches!(
            validate_space_name("SYSTEM"),
            Err(SpaceNameValidationError::ReservedName(_))
        ));
        assert!(matches!(
            validate_space_name("information_schema"),
            Err(SpaceNameValidationError::ReservedName(_))
        ));
    }
}
