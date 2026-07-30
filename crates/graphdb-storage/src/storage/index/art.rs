use std::fmt;
use std::mem;

/// An Adaptive Radix Tree based ordered map.
///
/// Keys are byte slices (`&[u8]`). ART compresses shared key prefixes in
/// internal nodes, providing better memory efficiency than BTreeMap for keys
/// with common prefixes (e.g. `space_id + index_type + value` composite keys).
pub struct ArtTree<V> {
    root: Option<ArtNode<V>>,
    len: usize,
}

impl<V> ArtTree<V> {
    pub fn new() -> Self {
        Self { root: None, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn insert(&mut self, key: &[u8], value: V) -> Option<V>
    where
        V: Default,
    {
        let old = match self.root.as_mut() {
            Some(root) => Self::insert_recursive(root, key, 0, value),
            None => {
                self.root = Some(ArtNode::leaf(key, value));
                None
            }
        };
        if old.is_none() {
            self.len += 1;
        }
        old
    }

    pub fn get(&self, key: &[u8]) -> Option<&V> {
        self.root
            .as_ref()
            .and_then(|root| Self::lookup(root, key, 0))
    }

    fn lookup<'a>(node: &'a ArtNode<V>, key: &[u8], depth: usize) -> Option<&'a V> {
        match node {
            ArtNode::Leaf(leaf) => {
                if leaf_suffix_matches(leaf, key, depth) {
                    Some(&leaf.value)
                } else {
                    None
                }
            }
            ArtNode::Internal(internal) => {
                let prefix = &internal.prefix;
                let remaining = &key[depth..];
                if remaining.len() < prefix.len() || &remaining[..prefix.len()] != prefix {
                    return None;
                }
                let next_depth = depth + prefix.len();
                if next_depth >= key.len() {
                    return internal.value.as_ref();
                }
                let byte = key[next_depth];
                internal
                    .child(byte)
                    .and_then(|c| Self::lookup(c, key, next_depth + 1))
            }
        }
    }

    fn insert_recursive(
        node: &mut ArtNode<V>,
        key: &[u8],
        depth: usize,
        value: V,
    ) -> Option<V>
    where
        V: Default,
    {
        match node {
            ArtNode::Leaf(leaf) => {
                let existing = &leaf.suffix;
                let remaining = &key[depth..];
                let common = common_prefix_len(existing, remaining);

                if common == existing.len() && common == remaining.len() {
                    let old = mem::replace(&mut leaf.value, value);
                    return Some(old);
                }

                let leaf_value = mem::take(&mut leaf.value);

                if common == existing.len() {
                    // existing key is prefix of new key: convert leaf to internal node
                    let mut internal = InternalNode::new(existing.to_vec());
                    internal.value = Some(leaf_value);
                    let new_byte = remaining[common];
                    internal.add_child(
                        new_byte,
                        ArtNode::Leaf(Leaf::new(remaining[common + 1..].to_vec(), value)),
                    );
                    *node = ArtNode::Internal(Box::new(internal));
                    return None;
                }

                if common == remaining.len() {
                    // new key is prefix of existing key: convert leaf to internal node
                    let mut internal = InternalNode::new(remaining.to_vec());
                    internal.value = Some(value);
                    let leaf_byte = existing[common];
                    internal.add_child(
                        leaf_byte,
                        ArtNode::Leaf(Leaf::new(existing[common + 1..].to_vec(), leaf_value)),
                    );
                    *node = ArtNode::Internal(Box::new(internal));
                    return None;
                }

                // Neither is prefix of the other: general split
                let shared = remaining[..common].to_vec();
                let leaf_byte = existing[common];
                let new_byte = remaining[common];
                let mut internal = InternalNode::new(shared);
                internal.add_child(
                    leaf_byte,
                    ArtNode::Leaf(Leaf::new(existing[common + 1..].to_vec(), leaf_value)),
                );
                internal.add_child(
                    new_byte,
                    ArtNode::Leaf(Leaf::new(remaining[common + 1..].to_vec(), value)),
                );
                *node = ArtNode::Internal(Box::new(internal));
                None
            }
            ArtNode::Internal(internal) => {
                let remaining = &key[depth..];
                let prefix = &internal.prefix;
                let common = common_prefix_len(prefix, remaining);

                if common < prefix.len() {
                    if common == remaining.len() {
                        // New key is a prefix of this internal node's prefix
                        let shared = remaining.to_vec();
                        let old_remainder = prefix[common..].to_vec();
                        let old_byte = old_remainder[0];
                        let old_prefix = old_remainder[1..].to_vec();
                        let old_children = mem::take(&mut internal.children);
                        let old_val = internal.value.take();
                        let old_node = ArtNode::Internal(Box::new(InternalNode {
                            prefix: old_prefix,
                            value: old_val,
                            children: old_children,
                        }));
                        let mut new_internal = InternalNode::new(shared);
                        new_internal.value = Some(value);
                        new_internal.add_child(old_byte, old_node);
                        *node = ArtNode::Internal(Box::new(new_internal));
                        return None;
                    }

                    let shared = remaining[..common].to_vec();
                    let old_remainder = prefix[common..].to_vec();
                    let new_suffix = remaining[common + 1..].to_vec();
                    let old_byte = old_remainder[0];
                    let new_byte = remaining[common];

                    let old_prefix = old_remainder[1..].to_vec();
                    let old_children = mem::take(&mut internal.children);
                    let old_value = internal.value.take();
                    let old_node = ArtNode::Internal(Box::new(InternalNode {
                        prefix: old_prefix,
                        value: old_value,
                        children: old_children,
                    }));
                    let mut new_internal = InternalNode::new(shared);
                    new_internal.add_child(old_byte, old_node);
                    new_internal.add_child(
                        new_byte,
                        ArtNode::Leaf(Leaf::new(new_suffix, value)),
                    );
                    *node = ArtNode::Internal(Box::new(new_internal));
                    return None;
                }

                let next_depth = depth + prefix.len();
                if next_depth >= key.len() {
                    let old = internal.value.replace(value);
                    return old;
                }
                let byte = key[next_depth];
                match internal.children.binary_search_by(|(k, _)| k.cmp(&byte)) {
                    Ok(i) => {
                        // Existing child: recurse to handle leaf/internal split
                        Self::insert_recursive(
                            &mut internal.children[i].1,
                            key,
                            next_depth + 1,
                            value,
                        )
                    }
                    Err(i) => {
                        // No existing child: create new leaf
                        let suffix = if next_depth + 1 >= key.len() {
                            Vec::new()
                        } else {
                            key[next_depth + 1..].to_vec()
                        };
                        internal
                            .children
                            .insert(i, (byte, ArtNode::Leaf(Leaf::new(suffix, value))));
                        None
                    }
                }
            }
        }
    }

    pub fn iter(&self) -> Vec<(Vec<u8>, &V)> {
        let mut result = Vec::with_capacity(self.len);
        if let Some(root) = &self.root {
            Self::collect_in_order(root, Vec::new(), &mut result);
        }
        result
    }

    fn collect_in_order<'a>(
        node: &'a ArtNode<V>,
        prefix: Vec<u8>,
        result: &mut Vec<(Vec<u8>, &'a V)>,
    ) {
        match node {
            ArtNode::Leaf(leaf) => {
                let full_key = reconstruct_key(&prefix, &leaf.suffix);
                result.push((full_key, &leaf.value));
            }
            ArtNode::Internal(internal) => {
                let mut base = prefix;
                base.extend_from_slice(&internal.prefix);
                if let Some(val) = &internal.value {
                    result.push((base.clone(), val));
                }
                for (byte, child_node) in internal.sorted_children() {
                    let mut child_prefix = base.clone();
                    child_prefix.push(byte);
                    Self::collect_in_order(child_node, child_prefix, result);
                }
            }
        }
    }

    pub fn range(&self, lower: &[u8], upper: &[u8]) -> Vec<(Vec<u8>, &V)> {
        let mut result = Vec::new();
        if let Some(root) = &self.root {
            Self::range_recursive(root, Vec::new(), lower, upper, &mut result);
        }
        result
    }

    fn range_recursive<'a>(
        node: &'a ArtNode<V>,
        prefix: Vec<u8>,
        lower: &[u8],
        upper: &[u8],
        result: &mut Vec<(Vec<u8>, &'a V)>,
    ) {
        match node {
            ArtNode::Leaf(leaf) => {
                let full_key = reconstruct_key(&prefix, &leaf.suffix);
                if full_key.as_slice() >= lower && full_key.as_slice() < upper {
                    result.push((full_key, &leaf.value));
                }
            }
            ArtNode::Internal(internal) => {
                let mut base = prefix;
                base.extend_from_slice(&internal.prefix);

                if let Some(val) = &internal.value {
                    let fk = base.clone();
                    if fk.as_slice() >= lower && fk.as_slice() < upper {
                        result.push((fk, val));
                    }
                }

                for (byte, child) in internal.sorted_children() {
                    let mut child_prefix = base.clone();
                    child_prefix.push(byte);

                    if child_prefix.as_slice() >= upper {
                        break;
                    }
                    let max_prefix = max_key_for_prefix(&child_prefix);
                    if max_prefix.as_slice() < lower {
                        continue;
                    }
                    Self::range_recursive(child, child_prefix, lower, upper, result);
                }
            }
        }
    }

    pub fn memory_usage(&self) -> usize {
        let base = mem::size_of::<Self>();
        match &self.root {
            Some(root) => base + Self::node_memory(root),
            None => base,
        }
    }

    fn node_memory(node: &ArtNode<V>) -> usize {
        let base = mem::size_of::<ArtNode<V>>();
        match node {
            ArtNode::Leaf(leaf) => base + leaf.suffix.capacity() + mem::size_of::<V>(),
            ArtNode::Internal(internal) => {
                let child_size: usize = internal
                    .children
                    .iter()
                    .map(|(_, c)| Self::node_memory(c))
                    .sum();
                base + mem::size_of::<InternalNode<V>>()
                    + internal.prefix.capacity()
                    + internal
                        .children
                        .iter()
                        .map(|(_, c)| mem::size_of_val(c))
                        .sum::<usize>()
                    + child_size
            }
        }
    }

    pub fn into_sorted_vec(mut self) -> Vec<(Vec<u8>, V)>
    where
        V: Default,
    {
        let mut result = Vec::with_capacity(self.len);
        if let Some(root) = self.root.take() {
            Self::collect_in_order_owned(root, Vec::new(), &mut result);
        }
        result
    }

    fn collect_in_order_owned(
        node: ArtNode<V>,
        prefix: Vec<u8>,
        result: &mut Vec<(Vec<u8>, V)>,
    ) where
        V: Default,
    {
        match node {
            ArtNode::Leaf(leaf) => {
                let full_key = reconstruct_key(&prefix, &leaf.suffix);
                result.push((full_key, leaf.value));
            }
            ArtNode::Internal(mut internal) => {
                let base = {
                    let mut b = prefix;
                    b.extend_from_slice(&internal.prefix);
                    b
                };
                if let Some(val) = internal.value.take() {
                    result.push((base.clone(), val));
                }
                let children = mem::take(&mut internal.children);
                for (byte, child) in children {
                    let mut child_prefix = base.clone();
                    child_prefix.push(byte);
                    Self::collect_in_order_owned(child, child_prefix, result);
                }
            }
        }
    }

    pub fn from_sorted_pairs(pairs: Vec<(Vec<u8>, V)>) -> Self
    where
        V: Default,
    {
        let mut tree = Self::new();
        for (key, value) in pairs {
            tree.insert(&key, value);
        }
        tree
    }
}

impl<V> Default for ArtTree<V> {
    fn default() -> Self {
        Self::new()
    }
}

// ── Node types ──

struct Leaf<V> {
    suffix: Vec<u8>,
    value: V,
}

impl<V> Leaf<V> {
    fn new(suffix: Vec<u8>, value: V) -> Self {
        Self { suffix, value }
    }
}

struct InternalNode<V> {
    prefix: Vec<u8>,
    value: Option<V>,
    children: Vec<(u8, ArtNode<V>)>,
}

impl<V> InternalNode<V> {
    fn new(prefix: Vec<u8>) -> Self {
        Self {
            prefix,
            value: None,
            children: Vec::new(),
        }
    }

    fn child(&self, byte: u8) -> Option<&ArtNode<V>> {
        self.children
            .binary_search_by(|(k, _)| k.cmp(&byte))
            .ok()
            .map(|i| &self.children[i].1)
    }

    fn add_child(&mut self, byte: u8, child: ArtNode<V>) {
        match self.children.binary_search_by(|(k, _)| k.cmp(&byte)) {
            Ok(i) => self.children[i].1 = child,
            Err(i) => self.children.insert(i, (byte, child)),
        }
    }

    fn sorted_children(&self) -> impl Iterator<Item = (u8, &ArtNode<V>)> {
        self.children.iter().map(|(k, v)| (*k, v))
    }
}

// ── Node enum ──

enum ArtNode<V> {
    Leaf(Leaf<V>),
    Internal(Box<InternalNode<V>>),
}

impl<V> ArtNode<V> {
    fn leaf(key: &[u8], value: V) -> Self {
        ArtNode::Leaf(Leaf::new(key.to_vec(), value))
    }
}

impl<V: fmt::Debug> fmt::Debug for ArtNode<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArtNode::Leaf(leaf) => f
                .debug_struct("Leaf")
                .field("suffix", &leaf.suffix)
                .field("value", &leaf.value)
                .finish(),
            ArtNode::Internal(internal) => f
                .debug_struct("Internal")
                .field("prefix", &internal.prefix)
                .field("has_value", &internal.value.is_some())
                .field("children", &internal.children.len())
                .finish(),
        }
    }
}

// ── Helpers ──

fn common_prefix_len(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

fn leaf_suffix_matches(leaf: &Leaf<impl Sized>, key: &[u8], depth: usize) -> bool {
    let suffix = &leaf.suffix;
    key.len() - depth == suffix.len() && key[depth..] == *suffix
}

fn reconstruct_key(prefix: &[u8], suffix: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + suffix.len());
    key.extend_from_slice(prefix);
    key.extend_from_slice(suffix);
    key
}

fn max_key_for_prefix(prefix: &[u8]) -> Vec<u8> {
    let mut key = prefix.to_vec();
    key.push(0xFF);
    key
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tree(pairs: &[(&[u8], i32)]) -> ArtTree<i32> {
        let mut tree = ArtTree::new();
        for (k, v) in pairs {
            tree.insert(k, *v);
        }
        tree
    }

    #[test]
    fn test_empty_tree() {
        let tree: ArtTree<i32> = ArtTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
        assert!(tree.get(b"anything").is_none());
    }

    #[test]
    fn test_insert_and_get() {
        let mut tree = ArtTree::new();
        assert!(tree.insert(b"foo", 42).is_none());
        assert_eq!(tree.get(b"foo"), Some(&42));
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn test_insert_update() {
        let mut tree = ArtTree::new();
        tree.insert(b"key", 1);
        assert_eq!(tree.insert(b"key", 2), Some(1));
        assert_eq!(tree.get(b"key"), Some(&2));
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn test_multiple_keys() {
        let mut tree = ArtTree::new();
        tree.insert(b"dog", 1);
        tree.insert(b"doge", 2);
        tree.insert(b"cat", 3);
        tree.insert(b"castle", 4);
        assert_eq!(tree.get(b"dog"), Some(&1));
        assert_eq!(tree.get(b"doge"), Some(&2));
        assert_eq!(tree.get(b"cat"), Some(&3));
        assert_eq!(tree.get(b"castle"), Some(&4));
        assert_eq!(tree.get(b"ca"), None);
        assert_eq!(tree.len(), 4);
    }

    #[test]
    fn test_shared_prefix() {
        let mut tree = ArtTree::new();
        tree.insert(b"abcdef", 1);
        tree.insert(b"abcxyz", 2);
        tree.insert(b"abc", 3);
        assert_eq!(tree.get(b"abcdef"), Some(&1));
        assert_eq!(tree.get(b"abcxyz"), Some(&2));
        assert_eq!(tree.get(b"abc"), Some(&3));
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn test_iter_order() {
        let tree = make_tree(&[(b"z", 26), (b"a", 1), (b"m", 13), (b"c", 3)]);
        let entries: Vec<_> = tree.iter().into_iter().map(|(k, v)| (k, *v)).collect();
        assert_eq!(
            entries,
            vec![
                (b"a".to_vec(), 1),
                (b"c".to_vec(), 3),
                (b"m".to_vec(), 13),
                (b"z".to_vec(), 26),
            ]
        );
    }

    #[test]
    fn test_iter_same_prefix() {
        let tree = make_tree(&[
            (b"foo:bar", 1),
            (b"foo:baz", 2),
            (b"foo:qux", 3),
        ]);
        let entries: Vec<_> = tree.iter().into_iter().map(|(k, v)| (k, *v)).collect();
        assert_eq!(
            entries,
            vec![
                (b"foo:bar".to_vec(), 1),
                (b"foo:baz".to_vec(), 2),
                (b"foo:qux".to_vec(), 3),
            ]
        );
    }

    #[test]
    fn test_range_scan() {
        let tree = make_tree(&[(b"a", 1), (b"b", 2), (b"c", 3), (b"d", 4), (b"e", 5)]);
        let entries: Vec<_> = tree
            .range(b"b", b"d")
            .into_iter()
            .map(|(k, v)| (k, *v))
            .collect();
        assert_eq!(entries, vec![(b"b".to_vec(), 2), (b"c".to_vec(), 3)]);
    }

    #[test]
    fn test_range_scan_prefix() {
        let tree = make_tree(&[
            (b"apple", 1),
            (b"application", 2),
            (b"apprentice", 3),
            (b"banana", 4),
        ]);
        let entries: Vec<_> = tree
            .range(b"app", b"banana")
            .into_iter()
            .map(|(k, v)| (k, *v))
            .collect();
        assert_eq!(
            entries,
            vec![
                (b"apple".to_vec(), 1),
                (b"application".to_vec(), 2),
                (b"apprentice".to_vec(), 3),
            ]
        );
    }

    #[test]
    fn test_into_sorted_vec() {
        let tree = make_tree(&[(b"z", 3), (b"a", 1), (b"m", 2)]);
        let vec = tree.into_sorted_vec();
        assert_eq!(
            vec,
            vec![(b"a".to_vec(), 1), (b"m".to_vec(), 2), (b"z".to_vec(), 3)]
        );
    }

    #[test]
    fn test_from_sorted_pairs() {
        let pairs = vec![
            (b"a".to_vec(), 1),
            (b"b".to_vec(), 2),
            (b"c".to_vec(), 3),
        ];
        let tree = ArtTree::from_sorted_pairs(pairs);
        assert_eq!(tree.len(), 3);
        assert_eq!(tree.get(b"b"), Some(&2));
    }

    #[test]
    fn test_large_number_of_keys() {
        let mut tree = ArtTree::new();
        let n = 1000;
        for i in 0..n {
            let key = format!("key_{:06}", i);
            tree.insert(key.as_bytes(), i);
        }
        assert_eq!(tree.len(), n);
        for i in 0..n {
            let key = format!("key_{:06}", i);
            assert_eq!(tree.get(key.as_bytes()), Some(&i));
        }
        let entries = tree.iter();
        for i in 1..entries.len() {
            assert!(entries[i - 1].0 <= entries[i].0);
        }
    }

    #[test]
    fn test_prefix_at_root_level() {
        let mut tree = ArtTree::new();
        tree.insert(b"prefix:key1", 10);
        tree.insert(b"prefix:key2", 20);
        tree.insert(b"prefix:key3", 30);
        assert_eq!(tree.len(), 3);
        assert_eq!(tree.get(b"prefix:key1"), Some(&10));
        assert_eq!(tree.get(b"prefix:key3"), Some(&30));
        assert_eq!(tree.get(b"prefix:key4"), None);
    }

    #[test]
    fn test_key_is_prefix_of_another() {
        let mut tree = ArtTree::new();
        tree.insert(b"test", 1);
        tree.insert(b"testing", 2);
        assert_eq!(tree.get(b"test"), Some(&1));
        assert_eq!(tree.get(b"testing"), Some(&2));
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn test_empty_key() {
        let mut tree = ArtTree::new();
        tree.insert(b"", 1);
        tree.insert(b"a", 2);
        assert_eq!(tree.get(b""), Some(&1));
        assert_eq!(tree.get(b"a"), Some(&2));
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn test_multiple_prefixes() {
        let mut tree = ArtTree::new();
        tree.insert(b"a.b.c", 1);
        tree.insert(b"a.b", 2);
        tree.insert(b"a", 3);
        assert_eq!(tree.get(b"a.b.c"), Some(&1));
        assert_eq!(tree.get(b"a.b"), Some(&2));
        assert_eq!(tree.get(b"a"), Some(&3));
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn test_nested_prefix_splits() {
        let mut tree = ArtTree::new();
        tree.insert(b"test.data.1", 1);
        tree.insert(b"test.data.2", 2);
        tree.insert(b"test.data.10", 10);
        tree.insert(b"test.meta.1", 100);
        assert_eq!(tree.len(), 4);
        let entries: Vec<_> = tree.iter().into_iter().map(|(k, v)| (k, *v)).collect();
        assert_eq!(entries[0], (b"test.data.1".to_vec(), 1));
        assert_eq!(entries[1], (b"test.data.10".to_vec(), 10));
        assert_eq!(entries[2], (b"test.data.2".to_vec(), 2));
        assert_eq!(entries[3], (b"test.meta.1".to_vec(), 100));
    }

    #[test]
    fn test_zero_byte_in_key() {
        let mut tree = ArtTree::new();
        tree.insert(b"\x00abc", 1);
        tree.insert(b"\x00xyz", 2);
        tree.insert(b"abc", 3);
        assert_eq!(tree.get(b"\x00abc"), Some(&1));
        assert_eq!(tree.get(b"\x00xyz"), Some(&2));
        assert_eq!(tree.get(b"abc"), Some(&3));
    }
}
