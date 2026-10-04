use crate::{ObjectId, ObjectStoreId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) blocks: Vec<ObjectId>,
}

impl FileEntry {
    pub(crate) fn new(path: String, size: u64, blocks: Vec<ObjectId>) -> Self {
        Self { path, size, blocks }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn blocks(&self) -> &[ObjectId] {
        &self.blocks
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRoot {
    pub(crate) store_id: ObjectStoreId,
    pub(crate) files: Vec<FileEntry>,
}

impl StateRoot {
    pub(crate) fn new(store_id: ObjectStoreId, files: Vec<FileEntry>) -> Self {
        Self { store_id, files }
    }

    pub fn store_id(&self) -> ObjectStoreId {
        self.store_id
    }

    pub fn files(&self) -> &[FileEntry] {
        &self.files
    }

    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.iter().find(|entry| entry.path == path)
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(store_id: ObjectStoreId, files: Vec<FileEntry>) -> Self {
        Self { store_id, files }
    }
}
