//! Customizing the Assertion Assist Module
//!
//! Provides common assertion functions in tests

#![allow(dead_code)]

/// Assertion results in success, return internal value
pub fn assert_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.expect("操作应该成功")
}

/// Asserts that the collection contains the specified number of elements
pub fn assert_count<T>(collection: &[T], expected: usize, item_name: &str) {
    assert_eq!(
        collection.len(),
        expected,
        "{}数量不匹配: 期望 {}, 实际 {}",
        item_name,
        expected,
        collection.len()
    );
}

/// Asserts that Option is Some and returns the internal value.
pub fn assert_some<T>(opt: &Option<T>) -> &T {
    opt.as_ref().expect("值应该是 Some")
}

/// Asserts that Option is None
pub fn assert_none<T>(opt: &Option<T>) {
    assert!(opt.is_none(), "The value should be None");
}
