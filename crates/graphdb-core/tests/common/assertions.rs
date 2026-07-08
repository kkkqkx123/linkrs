#![allow(dead_code)]

pub fn assert_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.expect("operation should succeed")
}

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

pub fn assert_some<T>(opt: &Option<T>) -> &T {
    opt.as_ref().expect("value should be Some")
}

pub fn assert_none<T>(opt: &Option<T>) {
    assert!(opt.is_none(), "the value should be None");
}
