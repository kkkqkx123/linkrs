//! Arena Allocator Module
//!
//! Provides efficient memory allocation for temporary data structures and batch operations
//! using the bumpalo crate as the underlying allocator.
//!
//! ## Overview
//!
//! Arena allocation is a memory management strategy where all allocations are made from
//! a contiguous region of memory (the "arena"), and all allocations can be freed at once
//! by resetting or dropping the arena. This provides:
//!
//! - **O(1) allocation**: Simply bump a pointer forward
//! - **O(1) deallocation**: Reset the entire arena at once
//! - **Cache locality**: Allocations are contiguous in memory
//! - **No fragmentation**: All allocations are from contiguous chunks
//!
//! ## When to Use
//!
//! Arena allocation is ideal for:
//!
//! 1. **Query execution**: Temporary results, intermediate values
//! 2. **Expression evaluation**: Temporary values during computation
//! 3. **Batch processing**: Processing large datasets with temporary buffers
//! 4. **Graph traversal**: Path accumulation, neighbor expansion
//! 5. **Parsing**: AST nodes with the same lifetime
//!
//! ## When NOT to Use
//!
//! Arena allocation is NOT suitable for:
//!
//! - Long-lived data structures (use `Box` or `Vec` instead)
//! - Data that needs individual deallocation
//! - Small, infrequent allocations
//!
//! ## Usage Examples
//!
//! ### Basic Usage
//!
//! ```rust,ignore
//! use graphdb::utils::Arena;
//!
//! let mut arena = Arena::new();
//!
//! // Allocate values
//! let a = arena.alloc(42);
//! let b = arena.alloc("hello");
//!
//! // Allocate slices
//! let slice = arena.alloc_slice(&[1, 2, 3, 4, 5]);
//!
//! // Allocate strings
//! let s = arena.alloc_str("world");
//!
//! // Reset to free all allocations at once
//! arena.reset();
//! ```
//!
//! ### With ArenaVec
//!
//! ```rust,ignore
//! use graphdb::utils::{Arena, ArenaVec};
//!
//! let arena = Arena::new();
//! let mut vec = ArenaVec::new(arena.inner());
//!
//! vec.push(1);
//! vec.push(2);
//! vec.push(3);
//!
//! // All elements are allocated in the arena
//! ```

use bumpalo::Bump;

/// Arena allocator for efficient batch allocations.
///
/// This is a wrapper around `bumpalo::Bump` providing a clean API
/// for arena-based memory allocation.
pub struct Arena {
    inner: Bump,
}

impl Arena {
    pub fn new() -> Self {
        Self { inner: Bump::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Bump::with_capacity(capacity),
        }
    }

    pub fn alloc<T>(&self, value: T) -> &mut T {
        self.inner.alloc(value)
    }

    pub fn alloc_slice<T: Copy>(&self, slice: &[T]) -> &mut [T] {
        self.inner.alloc_slice_copy(slice)
    }

    pub fn alloc_str(&self, s: &str) -> &mut str {
        self.inner.alloc_str(s)
    }

    pub fn allocated_bytes(&self) -> usize {
        self.inner.allocated_bytes()
    }

    pub fn reset(&mut self) {
        self.inner.reset();
    }

    pub fn inner(&self) -> &Bump {
        &self.inner
    }
}

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}

/// Arena pool for multi-threaded allocation.
///
/// Provides multiple arenas that can be distributed across threads
/// for concurrent allocation without contention.
pub struct ArenaPool {
    arenas: Vec<Arena>,
    current: std::sync::atomic::AtomicUsize,
}

impl ArenaPool {
    pub fn new(arena_count: usize) -> Self {
        let arenas = (0..arena_count).map(|_| Arena::new()).collect();
        Self {
            arenas,
            current: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn with_capacity(arena_count: usize, capacity: usize) -> Self {
        let arenas = (0..arena_count)
            .map(|_| Arena::with_capacity(capacity))
            .collect();
        Self {
            arenas,
            current: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn get_arena(&self) -> &Arena {
        use std::sync::atomic::Ordering;
        let idx = self.current.fetch_add(1, Ordering::Relaxed) % self.arenas.len();
        &self.arenas[idx]
    }

    pub fn reset_all(&mut self) {
        for arena in &mut self.arenas {
            arena.reset();
        }
    }

    pub fn total_allocated(&self) -> usize {
        self.arenas.iter().map(|a| a.allocated_bytes()).sum()
    }
}

/// Arena-based string tokenizer.
pub struct ArenaTokenizer<'a> {
    arena: &'a Bump,
}

impl<'a> ArenaTokenizer<'a> {
    pub fn new(arena: &'a Bump) -> Self {
        Self { arena }
    }

    pub fn tokenize(&self, text: &str) -> Vec<&'a str> {
        text.split_whitespace()
            .map(|word| self.arena.alloc_str(word) as &str)
            .collect()
    }

    pub fn tokenize_with_sep(&self, text: &str, sep: char) -> Vec<&'a str> {
        text.split(sep)
            .filter(|word| !word.is_empty())
            .map(|word| self.arena.alloc_str(word) as &str)
            .collect()
    }
}

/// Arena-based temporary vector.
pub struct ArenaVec<'a, T> {
    arena: &'a Bump,
    items: Vec<&'a mut T>,
}

impl<'a, T> ArenaVec<'a, T> {
    pub fn new(arena: &'a Bump) -> Self {
        Self {
            arena,
            items: Vec::new(),
        }
    }

    pub fn push(&mut self, value: T) {
        let item = self.arena.alloc(value);
        self.items.push(item);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.items.get(index).map(|item| &**item)
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter().map(|item| &**item)
    }
}

/// Arena-based string builder.
pub struct ArenaStringBuilder<'a> {
    arena: &'a Bump,
    buffer: Vec<&'a str>,
}

impl<'a> ArenaStringBuilder<'a> {
    pub fn new(arena: &'a Bump) -> Self {
        Self {
            arena,
            buffer: Vec::new(),
        }
    }

    pub fn append(&mut self, s: &str) {
        let slice = self.arena.alloc_str(s);
        self.buffer.push(slice);
    }

    pub fn build(&self) -> String {
        self.buffer.concat()
    }

    pub fn slices(&self) -> &[&'a str] {
        &self.buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arena_basic() {
        let mut arena = Arena::new();

        let a = arena.alloc(42);
        let b = arena.alloc("hello");

        assert_eq!(*a, 42);
        assert_eq!(*b, "hello");

        arena.reset();
        let c = arena.alloc(100);
        assert_eq!(*c, 100);
    }

    #[test]
    fn test_arena_slice() {
        let arena = Arena::new();

        let slice = arena.alloc_slice(&[1, 2, 3, 4, 5]);
        assert_eq!(slice.len(), 5);
        assert_eq!(slice[0], 1);
        assert_eq!(slice[4], 5);
    }

    #[test]
    fn test_arena_string() {
        let arena = Arena::new();

        let s = arena.alloc_str("hello world");
        assert_eq!(s, "hello world");
    }

    #[test]
    fn test_arena_pool() {
        let pool = ArenaPool::new(4);

        let arena1 = pool.get_arena();
        let _ = arena1.alloc(1);

        let arena2 = pool.get_arena();
        let _ = arena2.alloc(2);

        assert!(pool.total_allocated() > 0);
    }

    #[test]
    fn test_arena_tokenizer() {
        let arena = Arena::new();
        let tokenizer = ArenaTokenizer::new(arena.inner());

        let tokens = tokenizer.tokenize("hello world rust");
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0], "hello");
        assert_eq!(tokens[1], "world");
        assert_eq!(tokens[2], "rust");
    }

    #[test]
    fn test_arena_vec() {
        let arena = Arena::new();
        let mut vec = ArenaVec::new(arena.inner());

        vec.push(1);
        vec.push(2);
        vec.push(3);

        assert_eq!(vec.len(), 3);
        assert_eq!(vec.get(0), Some(&1));
        assert_eq!(vec.get(1), Some(&2));
        assert_eq!(vec.get(2), Some(&3));
    }

    #[test]
    fn test_arena_string_builder() {
        let arena = Arena::new();
        let mut builder = ArenaStringBuilder::new(arena.inner());

        builder.append("hello");
        builder.append(" ");
        builder.append("world");

        assert_eq!(builder.build(), "hello world");
    }
}
