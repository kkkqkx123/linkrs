use std::io::Read;

use graphdb_core::{StorageError, StorageResult};

pub struct BoundedReader<'a> {
    inner: &'a mut dyn Read,
    remaining: usize,
}

impl<'a> BoundedReader<'a> {
    pub fn new(inner: &'a mut impl Read, limit: usize) -> Self {
        Self {
            inner: inner as &mut dyn Read,
            remaining: limit,
        }
    }

    pub fn read_exact(&mut self, buf: &mut [u8]) -> StorageResult<()> {
        if buf.len() > self.remaining {
            return Err(StorageError::deserialize_error(format!(
                "read of {} bytes exceeds remaining bound of {}",
                buf.len(),
                self.remaining
            )));
        }
        self.inner.read_exact(buf)?;
        self.remaining -= buf.len();
        Ok(())
    }

    pub fn remaining(&self) -> usize {
        self.remaining
    }
}

impl Read for BoundedReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let limit = buf.len().min(self.remaining);
        let n = self.inner.read(&mut buf[..limit])?;
        self.remaining -= n;
        Ok(n)
    }
}
