use crate::core::types::{IndexGeneration, ManifestEpoch};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexShard {
    pub shard_id: u32,
    pub lower: Vec<u8>,
    pub upper: Vec<u8>,
    pub checkpoint_file: String,
}

impl IndexShard {
    pub fn validate(&self) -> Result<(), String> {
        if self.lower >= self.upper {
            return Err(format!(
                "Index shard {} has an empty or inverted range",
                self.shard_id
            ));
        }
        if self.checkpoint_file.is_empty() {
            return Err(format!(
                "Index shard {} has no checkpoint file",
                self.shard_id
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexManifest {
    pub index_id: u64,
    pub generation: IndexGeneration,
    pub epoch: ManifestEpoch,
    pub shards: Vec<IndexShard>,
}

impl IndexManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.shards.is_empty() {
            return Err("Index manifest must contain at least one shard".to_string());
        }
        for shard in &self.shards {
            shard.validate()?;
        }
        for pair in self.shards.windows(2) {
            if pair[0].upper != pair[1].lower {
                return Err(format!(
                    "Index shards {} and {} are not contiguous",
                    pair[0].shard_id, pair[1].shard_id
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{IndexManifest, IndexShard};
    use crate::core::types::{IndexGeneration, ManifestEpoch};

    #[test]
    fn manifest_rejects_range_gaps() {
        let manifest = IndexManifest {
            index_id: 1,
            generation: IndexGeneration::new(1),
            epoch: ManifestEpoch::new(1),
            shards: vec![
                IndexShard {
                    shard_id: 0,
                    lower: vec![0],
                    upper: vec![10],
                    checkpoint_file: "0.index".to_string(),
                },
                IndexShard {
                    shard_id: 1,
                    lower: vec![11],
                    upper: vec![20],
                    checkpoint_file: "1.index".to_string(),
                },
            ],
        };
        assert!(manifest.validate().is_err());
    }
}
