use bitvec::order::Lsb0;
use bitvec::vec::BitVec;

/// Overflow value chunk parallel to one topology overflow chunk.
///
/// Lengths are kept identical to the paired topology chunk; every topology
/// push/remove on the row applies the same operation here. Validity is one
/// bit per slot, matching the primary validity bitmap.
#[derive(Debug, Clone, Default)]
pub(crate) struct BundledOverflowValues {
    pub(crate) values: Vec<u64>,
    pub(crate) valid: BitVec<u8, Lsb0>,
}

impl BundledOverflowValues {
    pub(crate) fn with_capacity(cap: usize) -> Self {
        Self {
            values: Vec::with_capacity(cap),
            valid: BitVec::with_capacity(cap),
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

    #[allow(dead_code)]
    pub(crate) fn consolidated(values: &[u64], valid: &BitVec<u8, Lsb0>) -> Self {
        Self {
            values: values.to_vec(),
            valid: valid.clone(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn consolidated_from_slice(values: &[u64], valid: &[bool]) -> Self {
        let mut bits = BitVec::with_capacity(valid.len());
        for v in valid {
            bits.push(*v);
        }
        Self {
            values: values.to_vec(),
            valid: bits,
        }
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.values.len() * 8 + self.valid.len().div_ceil(8)
    }

    #[inline]
    pub(crate) fn get_valid(&self, index: usize) -> bool {
        self.valid.get(index).map(|b| *b).unwrap_or(false)
    }

    pub(crate) fn set_valid(&mut self, index: usize, value: bool) {
        if index < self.valid.len() {
            self.valid.set(index, value);
        }
    }

    #[allow(dead_code)]
    pub(crate) fn push_valid(&mut self, value: bool) {
        self.valid.push(value);
    }

    pub(crate) fn resize_valid(&mut self, len: usize, value: bool) {
        self.valid.resize(len, value);
    }
}
