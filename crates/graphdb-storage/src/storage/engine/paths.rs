use std::path::{Path, PathBuf};

#[cfg(test)]
use crate::core::types::LabelId;

/// Standard storage layout rooted at a database work directory.
#[derive(Debug, Clone)]
pub struct StoragePaths {
    root: PathBuf,
}

impl StoragePaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    pub fn wal_dir(&self) -> PathBuf {
        self.root.join("wal")
    }

    pub fn schema_dir(&self) -> PathBuf {
        self.root.join("schema")
    }

    pub fn schema_file(&self) -> PathBuf {
        self.schema_dir().join("schema.json")
    }

    pub fn index_meta_dir(&self) -> PathBuf {
        self.root.join("index_meta")
    }

    pub fn index_meta_file(&self) -> PathBuf {
        self.index_meta_dir().join("index_meta.json")
    }

    pub fn indexes_dir(&self) -> PathBuf {
        self.root.join("indexes")
    }

    pub fn version_file(&self) -> PathBuf {
        self.data_dir().join("version")
    }

    pub fn vertices_dir(&self) -> PathBuf {
        self.data_dir().join("vertices")
    }

    #[cfg(test)]
    pub fn vertex_dir(&self, label_id: LabelId) -> PathBuf {
        self.vertices_dir().join(format!("label_{}", label_id))
    }

    pub fn edges_dir(&self) -> PathBuf {
        self.data_dir().join("edges")
    }

    #[cfg(test)]
    pub fn edge_dir(&self, src_label: LabelId, dst_label: LabelId, edge_label: LabelId) -> PathBuf {
        self.edges_dir()
            .join(format!("{}_{}_{}", src_label, dst_label, edge_label))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_paths_layout() {
        let paths = StoragePaths::new("/tmp/graphdb");

        assert_eq!(paths.root(), Path::new("/tmp/graphdb"));
        assert_eq!(paths.data_dir(), PathBuf::from("/tmp/graphdb/data"));
        assert_eq!(paths.wal_dir(), PathBuf::from("/tmp/graphdb/wal"));
        assert_eq!(
            paths.schema_file(),
            PathBuf::from("/tmp/graphdb/schema/schema.json")
        );
        assert_eq!(
            paths.index_meta_file(),
            PathBuf::from("/tmp/graphdb/index_meta/index_meta.json")
        );
        assert_eq!(paths.indexes_dir(), PathBuf::from("/tmp/graphdb/indexes"));
        assert_eq!(
            paths.version_file(),
            PathBuf::from("/tmp/graphdb/data/version")
        );
        assert_eq!(
            paths.vertex_dir(7),
            PathBuf::from("/tmp/graphdb/data/vertices/label_7")
        );
        assert_eq!(
            paths.edge_dir(1, 2, 3),
            PathBuf::from("/tmp/graphdb/data/edges/1_2_3")
        );
    }
}
