//! Customizing the Assertion Assist Module
//!
//! Provides common assertion functions in tests

#![allow(dead_code)]

/// Assertion results in success, return internal value
pub fn assert_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.expect("Operation should succeed")
}

/// Asserts that the collection contains the specified number of elements
pub fn assert_count<T>(collection: &[T], expected: usize, item_name: &str) {
    assert_eq!(
        collection.len(),
        expected,
        "{} count mismatch: expected {}, got {}",
        item_name,
        expected,
        collection.len()
    );
}

/// Asserts that Option is Some and returns the internal value.
pub fn assert_some<T>(opt: &Option<T>) -> &T {
    opt.as_ref().expect("Value should be Some")
}

/// Asserts that Option is None
pub fn assert_none<T>(opt: &Option<T>) {
    assert!(opt.is_none(), "The value should be None");
}
