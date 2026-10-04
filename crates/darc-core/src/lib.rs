#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fmt;
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
            Self::HashCollision(_) => write!(formatter, "hash collision detected")
        }
    }
}

impl std::error::Error for ObjectStoreError {}

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

    pub const fn len(&self) -> usize {
        self.objects.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
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
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d,
            0xae, 0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10,
            0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
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
}
