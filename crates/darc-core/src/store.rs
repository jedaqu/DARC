use std::collections::BTreeMap;

use crate::codec::digest;
use crate::{Codec, DarcError, FileEntry, ObjectId, ObjectStoreId, StateRoot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub(crate) object_id: ObjectId,
    pub(crate) original_size: usize,
    pub(crate) compressed: Vec<u8>,
}

impl StoredObject {
    pub fn object_id(&self) -> ObjectId {
        self.object_id
    }

    pub fn original_size(&self) -> usize {
        self.original_size
    }

    pub fn compressed_size(&self) -> usize {
        self.compressed.len()
    }
}

pub struct ObjectStore<C: Codec> {
    store_id: ObjectStoreId,
    block_size: usize,
    codec: C,
    objects: BTreeMap<[u8; 32], StoredObject>,
}

impl<C: Codec> ObjectStore<C> {
    pub fn new(block_size: usize, codec: C) -> Self {
        Self::with_id(ObjectStoreId::new(), block_size, codec)
    }

    pub(crate) fn with_id(store_id: ObjectStoreId, block_size: usize, codec: C) -> Self {
        assert!(block_size > 0, "block_size must be positive");
        Self {
            store_id,
            block_size,
            codec,
            objects: BTreeMap::new(),
        }
    }

    pub fn store_id(&self) -> ObjectStoreId {
        self.store_id
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn intern(&mut self, data: &[u8]) -> Result<ObjectId, DarcError> {
        let digest = digest(data);
        if let Some(existing) = self.objects.get(&digest) {
            return Ok(existing.object_id);
        }

        let compressed = self.codec.encode(data)?;
        let object_id = ObjectId::new(self.store_id, digest);
        self.objects.insert(
            digest,
            StoredObject {
                object_id,
                original_size: data.len(),
                compressed,
            },
        );
        Ok(object_id)
    }

    pub fn get(&self, object_id: &ObjectId) -> Result<Vec<u8>, DarcError> {
        self.require_same_store(object_id)?;
        let stored = self
            .objects
            .get(object_id.digest())
            .ok_or(DarcError::ObjectNotFound)?;
        let data = self
            .codec
            .decode(&stored.compressed, stored.original_size)?;
        if digest(&data) != *object_id.digest() {
            return Err(DarcError::IntegrityFailure);
        }
        Ok(data)
    }

    pub fn physical_object_count(&self) -> usize {
        self.objects.len()
    }

    pub fn physical_compressed_bytes(&self) -> usize {
        self.objects
            .values()
            .map(StoredObject::compressed_size)
            .sum()
    }

    pub fn make_root(&mut self, files: &BTreeMap<String, Vec<u8>>) -> Result<StateRoot, DarcError> {
        let mut entries = Vec::with_capacity(files.len());
        for (path, data) in files {
            entries.push(self.make_entry(path, data)?);
        }
        Ok(StateRoot::new(self.store_id, entries))
    }

    pub fn replace_file(
        &mut self,
        root: &StateRoot,
        path: &str,
        data: &[u8],
    ) -> Result<StateRoot, DarcError> {
        self.require_root(root)?;
        let mut files = root.files.clone();
        if let Some(existing) = files.iter_mut().find(|entry| entry.path == path) {
            *existing = self.make_entry(path, data)?;
        } else {
            files.push(self.make_entry(path, data)?);
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(StateRoot::new(self.store_id, files))
    }

    pub fn remove_file(&self, root: &StateRoot, path: &str) -> Result<StateRoot, DarcError> {
        self.require_root(root)?;
        let files = root
            .files
            .iter()
            .filter(|entry| entry.path != path)
            .cloned()
            .collect();
        Ok(StateRoot::new(self.store_id, files))
    }

    pub fn materialize(&self, root: &StateRoot) -> Result<BTreeMap<String, Vec<u8>>, DarcError> {
        self.require_root(root)?;
        let mut result = BTreeMap::new();
        for entry in &root.files {
            let mut data = Vec::with_capacity(entry.size as usize);
            for block in &entry.blocks {
                data.extend_from_slice(&self.get(block)?);
            }
            if data.len() != entry.size as usize {
                return Err(DarcError::SizeMismatch);
            }
            result.insert(entry.path.clone(), data);
        }
        Ok(result)
    }

    pub fn verify_root(&self, root: &StateRoot) -> Result<(), DarcError> {
        self.require_root(root)?;
        for entry in &root.files {
            let mut size = 0usize;
            for block in &entry.blocks {
                size += self.get(block)?.len();
            }
            if size != entry.size as usize {
                return Err(DarcError::SizeMismatch);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn corrupt_for_test(&mut self, object_id: &ObjectId) -> Result<(), DarcError> {
        self.require_same_store(object_id)?;
        let stored = self
            .objects
            .get_mut(object_id.digest())
            .ok_or(DarcError::ObjectNotFound)?;
        if stored.compressed.is_empty() {
            return Err(DarcError::IntegrityFailure);
        }
        let last = stored.compressed.len() - 1;
        stored.compressed[last] ^= 0x01;
        Ok(())
    }

    fn make_entry(&mut self, path: &str, data: &[u8]) -> Result<FileEntry, DarcError> {
        let normalized = normalize_path(path)?;
        let mut blocks = Vec::new();
        for chunk in data.chunks(self.block_size) {
            blocks.push(self.intern(chunk)?);
        }
        if blocks.is_empty() {
            blocks.push(self.intern(&[])?);
        }
        Ok(FileEntry::new(normalized, data.len() as u64, blocks))
    }

    fn require_same_store(&self, object_id: &ObjectId) -> Result<(), DarcError> {
        if object_id.store_id() != self.store_id {
            return Err(DarcError::WrongStore);
        }
        Ok(())
    }

    fn require_root(&self, root: &StateRoot) -> Result<(), DarcError> {
        if root.store_id() != self.store_id {
            return Err(DarcError::WrongStore);
        }
        for entry in &root.files {
            for object_id in &entry.blocks {
                self.require_same_store(object_id)?;
            }
        }
        Ok(())
    }
}

fn normalize_path(path: &str) -> Result<String, DarcError> {
    let path = path.replace('\\', "/");
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Err(DarcError::InvalidPath);
    }
    if trimmed
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(DarcError::InvalidPath);
    }
    Ok(trimmed.to_string())
}
