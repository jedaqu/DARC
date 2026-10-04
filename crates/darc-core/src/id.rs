use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectStoreId([u8; 16]);

impl ObjectStoreId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().into_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl Default for ObjectStoreId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId {
    pub(crate) store_id: ObjectStoreId,
    pub(crate) digest: [u8; 32],
}

impl ObjectId {
    pub(crate) fn new(store_id: ObjectStoreId, digest: [u8; 32]) -> Self {
        Self { store_id, digest }
    }

    pub(crate) fn from_parts(store_id: ObjectStoreId, digest: [u8; 32]) -> Self {
        Self { store_id, digest }
    }

    pub fn store_id(&self) -> ObjectStoreId {
        self.store_id
    }

    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}
