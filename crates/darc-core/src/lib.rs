#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Arc;

use sha2::{Digest, Sha256};

/// Current public crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Identifies one object store.
///
/// The value is supplied by the caller so persisted stores can retain a stable
/// identity across process lifetimes without making the core library responsible
/// for ID generation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObjectStoreId(u128);

impl ObjectStoreId {
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u128 {
        self.0
    }
}

/// Opaque identity of an immutable object inside one object store.
///
/// Object IDs cannot be constructed directly outside this module. The store
/// creates them and all store access validates the embedded store identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObjectId {
    store: ObjectStoreId,
    digest: [u8; 32],
}

impl ObjectId {
    pub const fn store_id(self) -> ObjectStoreId {
        self.store
    }

    pub const fn digest(self) -> [u8; 32] {
        self.digest
    }
}

/// Identifies a store-bound object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    name: String,
    object: ObjectId,
}

impl FileEntry {
    pub fn new(name: impl Into<String>, object: ObjectId) -> Self {
        Self {
            name: name.into(),
            object,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn object(&self) -> ObjectId {
        self.object
    }
}

/// A lightweight immutable view of logical state.
///
/// A state root contains object references only. It does not own object bytes.
/// Creating a modified root clones the reference structure while leaving the
/// previous root unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateRoot {
    store: ObjectStoreId,
    entries: Vec<FileEntry>,
}

impl StateRoot {
    pub fn new(store: ObjectStoreId) -> Self {
        Self {
            store,
            entries: Vec::new(),
        }
    }

    pub fn from_entries(
        store: ObjectStoreId,
        entries: Vec<FileEntry>,
    ) -> Result<Self, StateRootError> {
        let mut names = std::collections::HashSet::with_capacity(entries.len());

        for entry in &entries {
            if entry.object.store_id() != store {
                return Err(StateRootError::StoreMismatch(StoreMismatch {
                    expected: store,
                    actual: entry.object.store_id(),
                }));
            }

            if !names.insert(entry.name()) {
                return Err(StateRootError::DuplicateName(entry.name.clone()));
            }
        }

        Ok(Self { store, entries })
    }

    pub const fn store_id(&self) -> ObjectStoreId {
        self.store
    }

    pub fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    pub fn add_file(
        &self,
        name: impl Into<String>,
        object: ObjectId,
    ) -> Result<Self, StateRootError> {
        self.validate_object(object)?;

        let name = name.into();
        if self.entries.iter().any(|entry| entry.name() == name) {
            return Err(StateRootError::DuplicateName(name));
        }

        let mut entries = self.entries.clone();
        entries.push(FileEntry::new(name, object));

        Ok(Self {
            store: self.store,
            entries,
        })
    }

    pub fn replace_object(&self, name: &str, object: ObjectId) -> Result<Self, StateRootError> {
        self.validate_object(object)?;

        let index = self
            .entries
            .iter()
            .position(|entry| entry.name() == name)
            .ok_or_else(|| StateRootError::MissingName(name.to_owned()))?;

        let mut entries = self.entries.clone();
        entries[index] = FileEntry::new(entries[index].name.clone(), object);

        Ok(Self {
            store: self.store,
            entries,
        })
    }

    /// Write a deterministic, versioned binary snapshot of this logical state.
    ///
    /// Entries are serialized in lexical name order so equivalent roots produce
    /// identical bytes regardless of insertion order. Object payloads are not
    /// included; only their content identities are referenced.
    pub fn write_snapshot<W: Write>(&self, mut writer: W) -> Result<(), StateRootPersistenceError> {
        writer.write_all(&STATE_ROOT_PERSISTENCE_MAGIC)?;
        writer.write_all(&STATE_ROOT_PERSISTENCE_VERSION.to_le_bytes())?;
        writer.write_all(&self.store.get().to_le_bytes())?;
        let entry_count = u64::try_from(self.entries.len())
            .map_err(|_| StateRootPersistenceError::LengthOverflow)?;
        writer.write_all(&entry_count.to_le_bytes())?;

        let mut entries: Vec<_> = self.entries.iter().collect();
        entries.sort_by(|left, right| left.name.cmp(&right.name));

        for entry in entries {
            let name = entry.name.as_bytes();
            let length =
                u64::try_from(name.len()).map_err(|_| StateRootPersistenceError::LengthOverflow)?;

            writer.write_all(&length.to_le_bytes())?;
            writer.write_all(name)?;
            writer.write_all(&entry.object.digest())?;
        }

        Ok(())
    }

    /// Load a logical state snapshot against an existing object store.
    ///
    /// The snapshot store identity must match the supplied store and every
    /// referenced object must already exist in that store.
    pub fn read_snapshot<R: Read>(
        store: &ObjectStore,
        mut reader: R,
    ) -> Result<Self, StateRootPersistenceError> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        let mut offset = 0usize;

        let magic = read_state_root_array::<8>(&data, &mut offset)?;
        if magic != STATE_ROOT_PERSISTENCE_MAGIC {
            return Err(StateRootPersistenceError::InvalidMagic);
        }

        let version = read_state_root_u16(&data, &mut offset)?;
        if version != STATE_ROOT_PERSISTENCE_VERSION {
            return Err(StateRootPersistenceError::UnsupportedVersion(version));
        }

        let snapshot_store = ObjectStoreId::new(u128::from_le_bytes(read_state_root_array::<16>(
            &data,
            &mut offset,
        )?));
        if snapshot_store != store.id() {
            return Err(StateRootPersistenceError::StoreMismatch(StoreMismatch {
                expected: store.id(),
                actual: snapshot_store,
            }));
        }

        let entry_count = read_state_root_u64(&data, &mut offset)?;
        let mut entries = Vec::new();

        for _ in 0..entry_count {
            let name_length = usize::try_from(read_state_root_u64(&data, &mut offset)?)
                .map_err(|_| StateRootPersistenceError::LengthOverflow)?;
            let end = offset
                .checked_add(name_length)
                .ok_or(StateRootPersistenceError::LengthOverflow)?;
            let name_bytes = data
                .get(offset..end)
                .ok_or(StateRootPersistenceError::Truncated)?;
            offset = end;

            let name = String::from_utf8(name_bytes.to_vec())
                .map_err(|_| StateRootPersistenceError::InvalidUtf8)?;
            let digest = read_state_root_array::<32>(&data, &mut offset)?;
            let object = ObjectId {
                store: snapshot_store,
                digest,
            };

            if !store.objects.contains_key(&object) {
                return Err(StateRootPersistenceError::UnknownObject(object));
            }

            entries.push(FileEntry::new(name, object));
        }

        if offset != data.len() {
            return Err(StateRootPersistenceError::TrailingBytes);
        }

        match StateRoot::from_entries(snapshot_store, entries) {
            Ok(root) => Ok(root),
            Err(StateRootError::StoreMismatch(error)) => {
                Err(StateRootPersistenceError::StoreMismatch(error))
            }
            Err(StateRootError::DuplicateName(name)) => {
                Err(StateRootPersistenceError::DuplicateName(name))
            }
            Err(StateRootError::MissingName(name)) => {
                unreachable!("from_entries cannot return MissingName: {name}")
            }
        }
    }

    fn validate_object(&self, object: ObjectId) -> Result<(), StateRootError> {
        if object.store_id() != self.store {
            return Err(StateRootError::StoreMismatch(StoreMismatch {
                expected: self.store,
                actual: object.store_id(),
            }));
        }

        Ok(())
    }
}

/// Errors raised when a state root crosses an object-store boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreMismatch {
    pub expected: ObjectStoreId,
    pub actual: ObjectStoreId,
}

impl fmt::Display for StoreMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "object belongs to store {}, expected store {}",
            self.actual.get(),
            self.expected.get()
        )
    }
}

impl std::error::Error for StoreMismatch {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StateRootError {
    StoreMismatch(StoreMismatch),
    DuplicateName(String),
    MissingName(String),
}

impl fmt::Display for StateRootError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StoreMismatch(error) => error.fmt(formatter),
            Self::DuplicateName(name) => write!(formatter, "file entry already exists: {name}"),
            Self::MissingName(name) => write!(formatter, "file entry does not exist: {name}"),
        }
    }
}

impl std::error::Error for StateRootError {}

/// Errors raised while serializing or loading a logical state snapshot.
#[derive(Debug)]
pub enum StateRootPersistenceError {
    Io(io::Error),
    InvalidMagic,
    UnsupportedVersion(u16),
    Truncated,
    LengthOverflow,
    InvalidUtf8,
    StoreMismatch(StoreMismatch),
    DuplicateName(String),
    UnknownObject(ObjectId),
    TrailingBytes,
}

impl fmt::Display for StateRootPersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::InvalidMagic => write!(formatter, "invalid state-root magic"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported state-root version: {version}")
            }
            Self::Truncated => write!(formatter, "truncated state-root snapshot"),
            Self::LengthOverflow => write!(formatter, "state-root length does not fit in memory"),
            Self::InvalidUtf8 => write!(formatter, "state-root entry name is not valid UTF-8"),
            Self::StoreMismatch(error) => error.fmt(formatter),
            Self::DuplicateName(name) => write!(formatter, "file entry already exists: {name}"),
            Self::UnknownObject(_) => write!(formatter, "state-root references an unknown object"),
            Self::TrailingBytes => write!(
                formatter,
                "unexpected trailing bytes in state-root snapshot"
            ),
        }
    }
}

impl std::error::Error for StateRootPersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::StoreMismatch(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for StateRootPersistenceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Errors returned by object-store access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectStoreError {
    StoreMismatch(StoreMismatch),
    UnknownObject(ObjectId),
    HashCollision(ObjectId),
}

impl fmt::Display for ObjectStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StoreMismatch(error) => error.fmt(formatter),
            Self::UnknownObject(_) => write!(formatter, "unknown object"),
            Self::HashCollision(_) => write!(formatter, "hash collision detected"),
        }
    }
}

impl std::error::Error for ObjectStoreError {}

/// Errors raised while serializing or loading an object-store snapshot.
#[derive(Debug)]
pub enum ObjectStorePersistenceError {
    Io(io::Error),
    InvalidMagic,
    UnsupportedVersion(u16),
    Truncated,
    LengthOverflow,
    DigestMismatch { object: ObjectId, actual: [u8; 32] },
    DuplicateObject(ObjectId),
    TrailingBytes,
}

impl fmt::Display for ObjectStorePersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::InvalidMagic => write!(formatter, "invalid object-store magic"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported object-store version: {version}")
            }
            Self::Truncated => write!(formatter, "truncated object-store snapshot"),
            Self::LengthOverflow => write!(formatter, "object length does not fit in memory"),
            Self::DigestMismatch { .. } => write!(formatter, "object digest mismatch"),
            Self::DuplicateObject(_) => write!(formatter, "duplicate object in snapshot"),
            Self::TrailingBytes => write!(formatter, "unexpected trailing bytes in snapshot"),
        }
    }
}

impl std::error::Error for ObjectStorePersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ObjectStorePersistenceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

const STATE_ROOT_PERSISTENCE_MAGIC: [u8; 8] = *b"DARCST01";
const STATE_ROOT_PERSISTENCE_VERSION: u16 = 1;

const PERSISTENCE_MAGIC: [u8; 8] = *b"DARCOS01";
const PERSISTENCE_VERSION: u16 = 1;

/// In-memory reference object store for the content-identity model.
///
/// Objects are immutable after insertion. Their identity is derived from the
/// SHA-256 digest of their exact bytes and is bound to the store identity.
#[derive(Debug)]
pub struct ObjectStore {
    id: ObjectStoreId,
    objects: HashMap<ObjectId, Arc<[u8]>>,
}

impl ObjectStore {
    pub fn new(id: ObjectStoreId) -> Self {
        Self {
            id,
            objects: HashMap::new(),
        }
    }

    pub const fn id(&self) -> ObjectStoreId {
        self.id
    }

    /// Intern bytes once and return their deterministic content identity.
    ///
    /// Equal bytes in the same store always produce the same object ID.
    /// A digest collision is treated as an explicit error rather than silently
    /// aliasing different bytes.
    pub fn intern(&mut self, bytes: impl AsRef<[u8]>) -> Result<ObjectId, ObjectStoreError> {
        let bytes = bytes.as_ref();
        let digest = Sha256::digest(bytes);
        let mut digest_bytes = [0u8; 32];
        digest_bytes.copy_from_slice(&digest);

        let object = ObjectId {
            store: self.id,
            digest: digest_bytes,
        };

        match self.objects.get(&object) {
            Some(existing) if existing.as_ref() == bytes => Ok(object),
            Some(_) => Err(ObjectStoreError::HashCollision(object)),
            None => {
                self.objects
                    .insert(object, Arc::<[u8]>::from(bytes.to_vec()));
                Ok(object)
            }
        }
    }

    pub fn get(&self, object: ObjectId) -> Result<&[u8], ObjectStoreError> {
        if object.store_id() != self.id {
            return Err(ObjectStoreError::StoreMismatch(StoreMismatch {
                expected: self.id,
                actual: object.store_id(),
            }));
        }

        self.objects
            .get(&object)
            .map(|bytes| bytes.as_ref())
            .ok_or(ObjectStoreError::UnknownObject(object))
    }

    /// Write a deterministic, versioned binary snapshot without compression.
    ///
    /// The snapshot stores the logical store identity and each object's digest plus
    /// original bytes. Objects are ordered by their content identity so equal stores
    /// serialize to the same bytes regardless of insertion order.
    pub fn write_snapshot<W: Write>(
        &self,
        mut writer: W,
    ) -> Result<(), ObjectStorePersistenceError> {
        writer.write_all(&PERSISTENCE_MAGIC)?;
        writer.write_all(&PERSISTENCE_VERSION.to_le_bytes())?;
        writer.write_all(&self.id.get().to_le_bytes())?;
        writer.write_all(&(self.objects.len() as u64).to_le_bytes())?;

        let mut objects: Vec<_> = self.objects.iter().collect();
        objects.sort_by_key(|(object_id, _)| **object_id);

        for (object_id, bytes) in objects {
            writer.write_all(&object_id.digest())?;
            writer.write_all(&(bytes.len() as u64).to_le_bytes())?;
            writer.write_all(bytes)?;
        }

        Ok(())
    }

    /// Load a versioned binary snapshot and verify every stored object digest.
    pub fn read_snapshot<R: Read>(mut reader: R) -> Result<Self, ObjectStorePersistenceError> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        let mut offset = 0usize;

        let magic = read_array::<8>(&data, &mut offset)?;
        if magic != PERSISTENCE_MAGIC {
            return Err(ObjectStorePersistenceError::InvalidMagic);
        }

        let version = read_u16(&data, &mut offset)?;
        if version != PERSISTENCE_VERSION {
            return Err(ObjectStorePersistenceError::UnsupportedVersion(version));
        }

        let store_id =
            ObjectStoreId::new(u128::from_le_bytes(read_array::<16>(&data, &mut offset)?));
        let object_count = read_u64(&data, &mut offset)?;
        let mut store = Self::new(store_id);

        for _ in 0..object_count {
            let digest = read_array::<32>(&data, &mut offset)?;
            let length = usize::try_from(read_u64(&data, &mut offset)?)
                .map_err(|_| ObjectStorePersistenceError::LengthOverflow)?;

            let end = offset
                .checked_add(length)
                .ok_or(ObjectStorePersistenceError::LengthOverflow)?;
            if end > data.len() {
                return Err(ObjectStorePersistenceError::Truncated);
            }

            let payload = &data[offset..end];
            offset = end;
            let actual = sha256(payload);
            let object_id = ObjectId {
                store: store_id,
                digest,
            };

            if actual != digest {
                return Err(ObjectStorePersistenceError::DigestMismatch {
                    object: object_id,
                    actual,
                });
            }

            if store.objects.contains_key(&object_id) {
                return Err(ObjectStorePersistenceError::DuplicateObject(object_id));
            }

            store
                .objects
                .insert(object_id, Arc::<[u8]>::from(payload.to_vec()));
        }

        if offset != data.len() {
            return Err(ObjectStorePersistenceError::TrailingBytes);
        }

        Ok(store)
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

fn read_array<const N: usize>(
    data: &[u8],
    offset: &mut usize,
) -> Result<[u8; N], ObjectStorePersistenceError> {
    let end = offset
        .checked_add(N)
        .ok_or(ObjectStorePersistenceError::Truncated)?;
    let slice = data
        .get(*offset..end)
        .ok_or(ObjectStorePersistenceError::Truncated)?;
    *offset = end;

    let mut result = [0u8; N];
    result.copy_from_slice(slice);
    Ok(result)
}

fn read_state_root_array<const N: usize>(
    data: &[u8],
    offset: &mut usize,
) -> Result<[u8; N], StateRootPersistenceError> {
    let end = offset
        .checked_add(N)
        .ok_or(StateRootPersistenceError::Truncated)?;
    let slice = data
        .get(*offset..end)
        .ok_or(StateRootPersistenceError::Truncated)?;
    *offset = end;

    let mut result = [0u8; N];
    result.copy_from_slice(slice);
    Ok(result)
}

fn read_state_root_u16(data: &[u8], offset: &mut usize) -> Result<u16, StateRootPersistenceError> {
    Ok(u16::from_le_bytes(read_state_root_array::<2>(
        data, offset,
    )?))
}

fn read_state_root_u64(data: &[u8], offset: &mut usize) -> Result<u64, StateRootPersistenceError> {
    Ok(u64::from_le_bytes(read_state_root_array::<8>(
        data, offset,
    )?))
}

fn read_u16(data: &[u8], offset: &mut usize) -> Result<u16, ObjectStorePersistenceError> {
    Ok(u16::from_le_bytes(read_array::<2>(data, offset)?))
}

fn read_u64(data: &[u8], offset: &mut usize) -> Result<u64, ObjectStorePersistenceError> {
    Ok(u64::from_le_bytes(read_array::<8>(data, offset)?))
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut result = [0u8; 32];
    result.copy_from_slice(&digest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ObjectStore {
        ObjectStore::new(ObjectStoreId::new(1))
    }

    #[test]
    fn version_is_non_empty() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn identical_content_is_interned_once() {
        let mut store = store();

        let first = store.intern(b"same").expect("intern succeeds");
        let second = store.intern(b"same").expect("intern succeeds");

        assert_eq!(first, second);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn distinct_content_gets_distinct_ids() {
        let mut store = store();

        let first = store.intern(b"alpha").expect("intern succeeds");
        let second = store.intern(b"beta").expect("intern succeeds");

        assert_ne!(first, second);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn empty_content_is_a_valid_interned_object() {
        let mut store = store();

        let first = store.intern([]).expect("intern succeeds");
        let second = store.intern([]).expect("intern succeeds");

        assert_eq!(first, second);
        assert_eq!(store.get(first).expect("object exists"), b"");
    }

    #[test]
    fn store_rejects_an_object_from_another_store() {
        let mut first = ObjectStore::new(ObjectStoreId::new(1));
        let second = ObjectStore::new(ObjectStoreId::new(2));
        let object = first.intern(b"data").expect("intern succeeds");

        assert_eq!(
            second.get(object),
            Err(ObjectStoreError::StoreMismatch(StoreMismatch {
                expected: ObjectStoreId::new(2),
                actual: ObjectStoreId::new(1),
            }))
        );
    }

    #[test]
    fn unknown_object_is_rejected() {
        let store = store();
        let fabricated_from_known_store = ObjectId {
            store: store.id(),
            digest: [0xAA; 32],
        };

        assert_eq!(
            store.get(fabricated_from_known_store),
            Err(ObjectStoreError::UnknownObject(fabricated_from_known_store))
        );
    }

    #[test]
    fn content_identity_is_deterministic_across_store_instances() {
        let mut first = store();
        let mut second = store();

        let a = first.intern(b"deterministic").expect("intern succeeds");
        let b = second.intern(b"deterministic").expect("intern succeeds");

        assert_eq!(a, b);
        assert_eq!(a.store_id(), ObjectStoreId::new(1));
    }

    #[test]
    fn known_sha256_digest_is_stable() {
        let mut store = store();
        let object = store.intern(b"abc").expect("intern succeeds");

        let expected = [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ];

        assert_eq!(object.digest(), expected);
        assert_eq!(object.digest(), sha256(b"abc"));
    }

    #[test]
    fn hash_collision_is_not_silently_aliased() {
        let mut store = store();
        let object = ObjectId {
            store: store.id(),
            digest: sha256(b"payload"),
        };
        store
            .objects
            .insert(object, Arc::<[u8]>::from(&b"different"[..]));

        assert_eq!(
            store.intern(b"payload"),
            Err(ObjectStoreError::HashCollision(object))
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn state_root_holds_references_without_copying_payloads() {
        let mut store = store();
        let object = store.intern(b"payload").expect("intern succeeds");

        let root = StateRoot::new(store.id())
            .add_file("a.txt", object)
            .expect("valid file entry");

        assert_eq!(root.entries()[0].object(), object);
        assert_eq!(root.entries()[0].name(), "a.txt");
        assert_eq!(store.get(object).expect("object exists"), b"payload");
    }

    #[test]
    fn replacing_an_object_creates_a_new_root_and_preserves_old_root() {
        let mut store = store();
        let old_object = store.intern(b"old").expect("intern succeeds");
        let new_object = store.intern(b"new").expect("intern succeeds");

        let root = StateRoot::new(store.id())
            .add_file("a.txt", old_object)
            .expect("valid file entry");
        let replaced = root
            .replace_object("a.txt", new_object)
            .expect("valid replacement");

        assert_eq!(root.entries()[0].object(), old_object);
        assert_eq!(replaced.entries()[0].object(), new_object);
        assert_ne!(root, replaced);
    }

    #[test]
    fn state_root_rejects_cross_store_objects() {
        let first = ObjectStore::new(ObjectStoreId::new(1));
        let mut second = ObjectStore::new(ObjectStoreId::new(2));
        let object = second.intern(b"foreign").expect("intern succeeds");

        let root = StateRoot::new(first.id());
        let error = root
            .add_file("foreign.txt", object)
            .expect_err("must reject");

        assert_eq!(
            error,
            StateRootError::StoreMismatch(StoreMismatch {
                expected: ObjectStoreId::new(1),
                actual: ObjectStoreId::new(2),
            })
        );
    }

    #[test]
    fn state_root_rejects_duplicate_names() {
        let mut store = store();
        let object = store.intern(b"data").expect("intern succeeds");
        let root = StateRoot::new(store.id())
            .add_file("a.txt", object)
            .expect("first entry");

        let error = root.add_file("a.txt", object).expect_err("must reject");

        assert_eq!(error, StateRootError::DuplicateName("a.txt".to_owned()));
    }

    #[test]
    fn state_root_rejects_unknown_names_on_replace() {
        let mut store = store();
        let object = store.intern(b"data").expect("intern succeeds");
        let root = StateRoot::new(store.id());

        let error = root
            .replace_object("missing.txt", object)
            .expect_err("must reject");

        assert_eq!(error, StateRootError::MissingName("missing.txt".to_owned()));
    }

    #[test]
    fn from_entries_validates_all_store_boundaries() {
        let mut first = ObjectStore::new(ObjectStoreId::new(1));
        let mut second = ObjectStore::new(ObjectStoreId::new(2));
        let first_object = first.intern(b"first").expect("intern succeeds");
        let second_object = second.intern(b"second").expect("intern succeeds");

        let error = StateRoot::from_entries(
            first.id(),
            vec![
                FileEntry::new("a", first_object),
                FileEntry::new("b", second_object),
            ],
        )
        .expect_err("mixed stores must be rejected");

        assert_eq!(
            error,
            StateRootError::StoreMismatch(StoreMismatch {
                expected: first.id(),
                actual: second.id(),
            })
        );
    }

    #[test]
    fn from_entries_rejects_duplicate_names() {
        let mut store = ObjectStore::new(ObjectStoreId::new(1));
        let object = store.intern(b"same").expect("intern succeeds");

        let error = StateRoot::from_entries(
            store.id(),
            vec![FileEntry::new("a", object), FileEntry::new("a", object)],
        )
        .expect_err("duplicate names must be rejected");

        assert_eq!(error, StateRootError::DuplicateName("a".to_owned()));
    }

    #[test]
    fn state_root_snapshot_round_trip_preserves_logical_state() {
        let mut store = ObjectStore::new(ObjectStoreId::new(7));
        let first = store.intern(b"alpha").expect("intern succeeds");
        let second = store.intern(b"beta").expect("intern succeeds");

        let root = StateRoot::new(store.id())
            .add_file("z.txt", second)
            .expect("first file")
            .add_file("a.txt", first)
            .expect("second file");

        let mut bytes = Vec::new();
        root.write_snapshot(&mut bytes).expect("snapshot writes");

        let restored = StateRoot::read_snapshot(&store, bytes.as_slice()).expect("snapshot reads");

        assert_eq!(restored, root);
    }

    #[test]
    fn state_root_snapshot_is_deterministic_across_insertion_order() {
        let mut store = ObjectStore::new(ObjectStoreId::new(7));
        let alpha = store.intern(b"alpha").expect("intern succeeds");
        let beta = store.intern(b"beta").expect("intern succeeds");

        let first = StateRoot::from_entries(
            store.id(),
            vec![
                FileEntry::new("b.txt", beta),
                FileEntry::new("a.txt", alpha),
            ],
        )
        .expect("first root");
        let second = StateRoot::from_entries(
            store.id(),
            vec![
                FileEntry::new("a.txt", alpha),
                FileEntry::new("b.txt", beta),
            ],
        )
        .expect("second root");

        let mut first_bytes = Vec::new();
        let mut second_bytes = Vec::new();
        first
            .write_snapshot(&mut first_bytes)
            .expect("snapshot writes");
        second
            .write_snapshot(&mut second_bytes)
            .expect("snapshot writes");

        assert_eq!(first_bytes, second_bytes);
    }

    #[test]
    fn state_root_snapshot_rejects_invalid_magic() {
        let root = StateRoot::new(ObjectStoreId::new(7));
        let mut bytes = Vec::new();
        root.write_snapshot(&mut bytes).expect("snapshot writes");
        bytes[0] ^= 0xFF;

        let store = ObjectStore::new(ObjectStoreId::new(7));
        assert!(matches!(
            StateRoot::read_snapshot(&store, bytes.as_slice()),
            Err(StateRootPersistenceError::InvalidMagic)
        ));
    }

    #[test]
    fn state_root_snapshot_rejects_unsupported_version() {
        let root = StateRoot::new(ObjectStoreId::new(7));
        let mut bytes = Vec::new();
        root.write_snapshot(&mut bytes).expect("snapshot writes");
        bytes[8..10].copy_from_slice(&2u16.to_le_bytes());

        let store = ObjectStore::new(ObjectStoreId::new(7));
        assert!(matches!(
            StateRoot::read_snapshot(&store, bytes.as_slice()),
            Err(StateRootPersistenceError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn state_root_snapshot_rejects_truncation() {
        let root = StateRoot::new(ObjectStoreId::new(7));
        let mut bytes = Vec::new();
        root.write_snapshot(&mut bytes).expect("snapshot writes");
        bytes.truncate(bytes.len() - 1);

        let store = ObjectStore::new(ObjectStoreId::new(7));
        assert!(matches!(
            StateRoot::read_snapshot(&store, bytes.as_slice()),
            Err(StateRootPersistenceError::Truncated)
        ));
    }

    #[test]
    fn state_root_snapshot_rejects_length_overflow() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"DARCST01");
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&7u128.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());

        let store = ObjectStore::new(ObjectStoreId::new(7));
        assert!(matches!(
            StateRoot::read_snapshot(&store, bytes.as_slice()),
            Err(StateRootPersistenceError::LengthOverflow)
        ));
    }

    #[test]
    fn state_root_snapshot_rejects_invalid_utf8() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"DARCST01");
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&7u128.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.push(0xFF);
        bytes.extend_from_slice(&[0; 32]);

        let store = ObjectStore::new(ObjectStoreId::new(7));
        assert!(matches!(
            StateRoot::read_snapshot(&store, bytes.as_slice()),
            Err(StateRootPersistenceError::InvalidUtf8)
        ));
    }

    #[test]
    fn state_root_snapshot_rejects_duplicate_names() {
        let mut store = ObjectStore::new(ObjectStoreId::new(7));
        let object = store.intern(b"same").expect("intern succeeds");
        let mut root = StateRoot::new(store.id());
        root.entries = vec![FileEntry::new("a", object), FileEntry::new("a", object)];

        let mut bytes = Vec::new();
        root.write_snapshot(&mut bytes).expect("snapshot writes");

        assert!(matches!(
            StateRoot::read_snapshot(&store, bytes.as_slice()),
            Err(StateRootPersistenceError::DuplicateName(name)) if name == "a"
        ));
    }

    #[test]
    fn state_root_snapshot_rejects_wrong_store_id() {
        let mut source = ObjectStore::new(ObjectStoreId::new(7));
        let object = source.intern(b"payload").expect("intern succeeds");
        let root = StateRoot::new(source.id())
            .add_file("a", object)
            .expect("root entry");

        let mut bytes = Vec::new();
        root.write_snapshot(&mut bytes).expect("snapshot writes");

        let target = ObjectStore::new(ObjectStoreId::new(8));
        assert!(matches!(
            StateRoot::read_snapshot(&target, bytes.as_slice()),
            Err(StateRootPersistenceError::StoreMismatch(StoreMismatch { expected, actual }))
                if expected == ObjectStoreId::new(8) && actual == ObjectStoreId::new(7)
        ));
    }

    #[test]
    fn state_root_snapshot_rejects_unknown_object() {
        let source = {
            let mut store = ObjectStore::new(ObjectStoreId::new(7));
            let object = store.intern(b"payload").expect("intern succeeds");
            StateRoot::new(store.id())
                .add_file("a", object)
                .expect("root entry")
        };

        let mut bytes = Vec::new();
        source.write_snapshot(&mut bytes).expect("snapshot writes");

        let target = ObjectStore::new(ObjectStoreId::new(7));
        assert!(matches!(
            StateRoot::read_snapshot(&target, bytes.as_slice()),
            Err(StateRootPersistenceError::UnknownObject(_))
        ));
    }

    #[test]
    fn state_root_snapshot_rejects_trailing_bytes() {
        let root = StateRoot::new(ObjectStoreId::new(7));
        let mut bytes = Vec::new();
        root.write_snapshot(&mut bytes).expect("snapshot writes");
        bytes.push(0);

        let store = ObjectStore::new(ObjectStoreId::new(7));
        assert!(matches!(
            StateRoot::read_snapshot(&store, bytes.as_slice()),
            Err(StateRootPersistenceError::TrailingBytes)
        ));
    }

    #[test]
    fn snapshot_round_trip_preserves_store_and_objects() {
        let mut store = ObjectStore::new(ObjectStoreId::new(7));
        let first = store.intern(b"alpha").expect("intern succeeds");
        let second = store.intern(b"beta").expect("intern succeeds");

        let mut bytes = Vec::new();
        store.write_snapshot(&mut bytes).expect("snapshot writes");

        let restored = ObjectStore::read_snapshot(bytes.as_slice()).expect("snapshot reads");

        assert_eq!(restored.id(), store.id());
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.get(first).expect("first exists"), b"alpha");
        assert_eq!(restored.get(second).expect("second exists"), b"beta");
    }

    #[test]
    fn snapshot_is_deterministic_across_insertion_order() {
        let mut first = ObjectStore::new(ObjectStoreId::new(7));
        first.intern(b"alpha").expect("intern succeeds");
        first.intern(b"beta").expect("intern succeeds");

        let mut second = ObjectStore::new(ObjectStoreId::new(7));
        second.intern(b"beta").expect("intern succeeds");
        second.intern(b"alpha").expect("intern succeeds");

        let mut first_bytes = Vec::new();
        let mut second_bytes = Vec::new();
        first
            .write_snapshot(&mut first_bytes)
            .expect("snapshot writes");
        second
            .write_snapshot(&mut second_bytes)
            .expect("snapshot writes");

        assert_eq!(first_bytes, second_bytes);
    }

    #[test]
    fn snapshot_rejects_invalid_magic() {
        let mut store = ObjectStore::new(ObjectStoreId::new(7));
        store.intern(b"payload").expect("intern succeeds");
        let mut bytes = Vec::new();
        store.write_snapshot(&mut bytes).expect("snapshot writes");
        bytes[0] ^= 0xFF;

        assert!(matches!(
            ObjectStore::read_snapshot(bytes.as_slice()),
            Err(ObjectStorePersistenceError::InvalidMagic)
        ));
    }

    #[test]
    fn snapshot_rejects_unsupported_version() {
        let mut store = ObjectStore::new(ObjectStoreId::new(7));
        store.intern(b"payload").expect("intern succeeds");
        let mut bytes = Vec::new();
        store.write_snapshot(&mut bytes).expect("snapshot writes");
        bytes[8..10].copy_from_slice(&2u16.to_le_bytes());

        assert!(matches!(
            ObjectStore::read_snapshot(bytes.as_slice()),
            Err(ObjectStorePersistenceError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn snapshot_rejects_truncation() {
        let mut store = ObjectStore::new(ObjectStoreId::new(7));
        store.intern(b"payload").expect("intern succeeds");
        let mut bytes = Vec::new();
        store.write_snapshot(&mut bytes).expect("snapshot writes");
        bytes.truncate(bytes.len() - 1);

        assert!(matches!(
            ObjectStore::read_snapshot(bytes.as_slice()),
            Err(ObjectStorePersistenceError::Truncated)
        ));
    }

    #[test]
    fn snapshot_rejects_digest_corruption() {
        let mut store = ObjectStore::new(ObjectStoreId::new(7));
        store.intern(b"payload").expect("intern succeeds");
        let mut bytes = Vec::new();
        store.write_snapshot(&mut bytes).expect("snapshot writes");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;

        assert!(matches!(
            ObjectStore::read_snapshot(bytes.as_slice()),
            Err(ObjectStorePersistenceError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn snapshot_rejects_duplicate_records() {
        let mut store = ObjectStore::new(ObjectStoreId::new(7));
        store.intern(b"payload").expect("intern succeeds");
        let mut bytes = Vec::new();
        store.write_snapshot(&mut bytes).expect("snapshot writes");
        let first_record = bytes[34..].to_vec();
        bytes[26..34].copy_from_slice(&2u64.to_le_bytes());
        bytes.extend_from_slice(&first_record);

        assert!(matches!(
            ObjectStore::read_snapshot(bytes.as_slice()),
            Err(ObjectStorePersistenceError::DuplicateObject(_))
        ));
    }

    #[test]
    fn snapshot_rejects_trailing_bytes() {
        let mut store = ObjectStore::new(ObjectStoreId::new(7));
        store.intern(b"payload").expect("intern succeeds");
        let mut bytes = Vec::new();
        store.write_snapshot(&mut bytes).expect("snapshot writes");
        bytes.push(0);

        assert!(matches!(
            ObjectStore::read_snapshot(bytes.as_slice()),
            Err(ObjectStorePersistenceError::TrailingBytes)
        ));
    }
}
