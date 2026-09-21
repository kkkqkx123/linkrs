use graphdb_core::StorageResult;

use super::super::csr_trait::CsrBase;
use super::super::{ImmutableCsr, MutableCsr, PureTopologyCsr, SingleMutableCsr};
use super::CsrVariant;

impl CsrBase for CsrVariant {
    fn vertex_capacity(&self) -> usize {
        match self {
            CsrVariant::None { vertex_capacity } => *vertex_capacity,
            CsrVariant::Multiple(csr) => csr.vertex_capacity(),
            CsrVariant::Single(csr) => csr.vertex_capacity(),
            CsrVariant::Pure(csr) => csr.vertex_capacity(),
            CsrVariant::Bundled(csr) => csr.vertex_capacity(),
            CsrVariant::Frozen(csr) => csr.vertex_capacity(),
            CsrVariant::Mapped(csr) => csr.vertex_capacity(),
        }
    }

    fn edge_count(&self) -> u64 {
        dispatch!(self, edge_count() -> 0)
    }

    fn dump(&self) -> Vec<u8> {
        match self {
            CsrVariant::None { vertex_capacity } => {
                let mut result = vec![0u8];
                result.extend((*vertex_capacity as u64).to_le_bytes());
                result
            }
            CsrVariant::Multiple(csr) => {
                let mut result = vec![1u8];
                result.extend(csr.dump());
                result
            }
            CsrVariant::Single(csr) => {
                let mut result = vec![2u8];
                result.extend(csr.dump());
                result
            }
            CsrVariant::Frozen(csr) => {
                let mut result = vec![3u8];
                result.extend(csr.dump());
                result
            }
            CsrVariant::Mapped(csr) => {
                let mut result = vec![3u8];
                result.extend(csr.dump());
                result
            }
            CsrVariant::Pure(csr) => {
                let mut result = vec![4u8];
                result.extend(csr.dump());
                result
            }
            CsrVariant::Bundled(csr) => {
                let mut result = vec![5u8];
                result.extend(csr.dump());
                result
            }
        }
    }

    fn dump_into(&self, out: &mut Vec<u8>) {
        match self {
            CsrVariant::None { vertex_capacity } => {
                out.push(0u8);
                out.extend((*vertex_capacity as u64).to_le_bytes());
            }
            CsrVariant::Multiple(csr) => {
                out.push(1u8);
                csr.dump_into(out);
            }
            CsrVariant::Single(csr) => {
                out.push(2u8);
                csr.dump_into(out);
            }
            CsrVariant::Frozen(csr) => {
                out.push(3u8);
                csr.dump_into(out);
            }
            CsrVariant::Mapped(csr) => {
                out.push(3u8);
                csr.dump_into(out);
            }
            CsrVariant::Pure(csr) => {
                out.push(4u8);
                csr.dump_into(out);
            }
            CsrVariant::Bundled(csr) => {
                out.push(5u8);
                csr.dump_into(out);
            }
        }
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.is_empty() {
            return Err(graphdb_core::StorageError::deserialize_error(
                "Cannot load CSR variant: empty data",
            ));
        }

        match data[0] {
            0 => {
                if data.len() < 9 {
                    return Err(graphdb_core::StorageError::deserialize_error(
                        "Cannot load None CSR variant: data too short",
                    ));
                }
                let vertex_capacity = u64::from_le_bytes([
                    data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
                ]) as usize;
                *self = CsrVariant::None { vertex_capacity };
                Ok(())
            }
            1 => {
                let mut csr = MutableCsr::new();
                csr.load(&data[1..])?;
                *self = CsrVariant::Multiple(Box::new(csr));
                Ok(())
            }
            2 => {
                let mut csr = SingleMutableCsr::new();
                csr.load(&data[1..])?;
                *self = CsrVariant::Single(csr);
                Ok(())
            }
            3 => {
                let mut csr = ImmutableCsr::new();
                csr.load(&data[1..])?;
                *self = CsrVariant::Frozen(Box::new(csr));
                Ok(())
            }
            4 => {
                let mut csr = PureTopologyCsr::default();
                csr.load(&data[1..])?;
                *self = CsrVariant::Pure(Box::new(csr));
                Ok(())
            }
            5 => {
                let mut csr = super::super::BundledCsr::default();
                csr.load(&data[1..])?;
                *self = CsrVariant::Bundled(Box::new(csr));
                Ok(())
            }
            _ => Err(graphdb_core::StorageError::deserialize_error(
                "Invalid CSR variant tag in serialized data",
            )),
        }
    }
}

impl CsrVariant {
    /// Borrow-based dump reusing caller-owned column buffers.
    ///
    /// Same bytes as `dump_into` through the base trait; a checkpoint over
    /// many groups pays one allocation per column instead of one per group.
    pub fn dump_into_with_scratch(
        &self,
        out: &mut Vec<u8>,
        scratch: &mut super::super::mutable_csr::persistence::CsrDumpScratch,
    ) {
        match self {
            CsrVariant::None { vertex_capacity } => {
                out.push(0u8);
                out.extend((*vertex_capacity as u64).to_le_bytes());
            }
            CsrVariant::Multiple(csr) => {
                out.push(1u8);
                csr.dump_into_with_scratch(out, scratch);
            }
            CsrVariant::Single(csr) => {
                out.push(2u8);
                csr.dump_into(out);
            }
            CsrVariant::Frozen(csr) => {
                out.push(3u8);
                csr.dump_into(out);
            }
            CsrVariant::Mapped(csr) => {
                out.push(3u8);
                csr.dump_into(out);
            }
            CsrVariant::Pure(csr) => {
                out.push(4u8);
                csr.dump_into(out);
            }
            CsrVariant::Bundled(csr) => {
                out.push(5u8);
                csr.dump_into(out);
            }
        }
    }
}
