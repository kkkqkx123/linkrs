use dashmap::DashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct StrInterner {
    inner: Arc<DashMap<String, Arc<str>>>,
}

impl StrInterner {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(DashMap::with_capacity(capacity)),
        }
    }

    pub fn intern(&self, s: &str) -> Arc<str> {
        if let Some(entry) = self.inner.get(s) {
            return Arc::clone(&entry);
        }
        let interned: Arc<str> = Arc::from(s);
        self.inner
            .insert(interned.to_string(), Arc::clone(&interned));
        interned
    }

    pub fn intern_string(&self, s: String) -> Arc<str> {
        if let Some(entry) = self.inner.get(s.as_str()) {
            return Arc::clone(&entry);
        }
        let interned: Arc<str> = Arc::from(s);
        self.inner
            .insert(interned.to_string(), Arc::clone(&interned));
        interned
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn clear(&self) {
        self.inner.clear();
    }
}

impl Default for StrInterner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intern_basic() {
        let interner = StrInterner::new();
        let a = interner.intern("hello world");
        let b = interner.intern("hello world");
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn test_intern_distinct() {
        let interner = StrInterner::new();
        let a = interner.intern("hello");
        let b = interner.intern("world");
        assert!(!Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn test_intern_string() {
        let interner = StrInterner::new();
        let s = String::from("test string");
        let a = interner.intern_string(s);
        let b = interner.intern("test string");
        assert!(Arc::ptr_eq(&a, &b));
    }
}
