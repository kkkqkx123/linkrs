/// Overflow value chunk parallel to one topology overflow chunk.
///
/// Lengths are kept identical to the paired topology chunk; every topology
/// push/remove on the row applies the same operation here.
#[derive(Debug, Clone, Default)]
pub(crate) struct BundledOverflowValues {
    pub(crate) values: Vec<u64>,
    pub(crate) valid: Vec<bool>,
}

impl BundledOverflowValues {
    pub(crate) fn with_capacity(cap: usize) -> Self {
        Self {
            values: Vec::with_capacity(cap),
            valid: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    #[inline]
    pub(crate) fn remove(&mut self, index: usize) {
        self.values.remove(index);
        self.valid.remove(index);
    }

    pub(crate) fn consolidated(values: &[u64], valid: &[bool]) -> Self {
        Self {
            values: values.to_vec(),
            valid: valid.to_vec(),
        }
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.values.len() * 8 + self.valid.len()
    }
}
