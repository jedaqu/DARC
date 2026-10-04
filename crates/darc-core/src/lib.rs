#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

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
    slot: u64,
}

impl ObjectId {
    pub const fn store_id(self) -> ObjectStoreId {
        self.store
    }

    pub const fn slot(self) -> u64 {
        self.slot
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
    ) -> Result<Self, StoreMismatch> {
        for entry in &entries {
            if entry.object.store_id() != store {
                return Err(StoreMismatch {
                    expected: store,
                    actual: entry.object.store_id(),
                });
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

    pub fn replace_object(
        &self,
        name: &str,
        object: ObjectId,
    ) -> Result<Self, StateRootError> {
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
}

impl fmt::Display for ObjectStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StoreMismatch(error) => error.fmt(formatter),
            Self::UnknownObject(object) => {
                write!(formatter, "unknown object at slot {}", object.slot())
            }
        }
    }
}

impl std::error::Error for ObjectStoreError {}

/// In-memory reference object store for the first public core model.
///
/// Objects are immutable after insertion. The equality index deliberately uses
/// the original bytes for Phase 1 correctness; persistent hashing and packed
/// storage are deferred to later phases.
#[derive(Debug)]
pub struct ObjectStore {
    id: ObjectStoreId,
    objects: Vec<Arc<[u8]>>,
    index: HashMap<Vec<u8>, ObjectId>,
}

impl ObjectStore {
    pub fn new(id: ObjectStoreId) -> Self {
        Self {
            id,
            objects: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub const fn id(&self) -> ObjectStoreId {
        self.id
    }

    /// Intern bytes once and return their stable object identity for this store.
    pub fn intern(&mut self, bytes: impl AsRef<[u8]>) -> ObjectId {
        let bytes = bytes.as_ref();

        if let Some(&object) = self.index.get(bytes) {
            return object;
        }

        let slot = u64::try_from(self.objects.len())
            .expect("object store exceeded the representable u64 slot count");
        let object = ObjectId {
            store: self.id,
            slot,
        };

        let owned = bytes.to_vec();
        self.objects.push(Arc::<[u8]>::from(owned.clone()));
        self.index.insert(owned, object);

        object
    }

    pub fn get(&self, object: ObjectId) -> Result<&[u8], ObjectStoreError> {
        if object.store_id() != self.id {
            return Err(ObjectStoreError::StoreMismatch(StoreMismatch {
                expected: self.id,
                actual: object.store_id(),
            }));
        }

        self.objects
            .get(object.slot() as usize)
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

        let first = store.intern(b"same");
        let second = store.intern(b"same");

        assert_eq!(first, second);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn distinct_content_gets_distinct_ids() {
        let mut store = store();

        let first = store.intern(b"alpha");
        let second = store.intern(b"beta");

        assert_ne!(first, second);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn empty_content_is_a_valid_interned_object() {
        let mut store = store();

        let first = store.intern([]);
        let second = store.intern([]);

        assert_eq!(first, second);
        assert_eq!(store.get(first).expect("object exists"), b"");
    }

    #[test]
    fn store_rejects_an_object_from_another_store() {
        let mut first = ObjectStore::new(ObjectStoreId::new(1));
        let second = ObjectStore::new(ObjectStoreId::new(2));
        let object = first.intern(b"data");

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
            slot: 99,
        };

        assert_eq!(
            store.get(fabricated_from_known_store),
            Err(ObjectStoreError::UnknownObject(fabricated_from_known_store))
        );
    }

    #[test]
    fn state_root_holds_references_without_copying_payloads() {
        let mut store = store();
        let object = store.intern(b"payload");

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
        let old_object = store.intern(b"old");
        let new_object = store.intern(b"new");

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
        let object = second.intern(b"foreign");

        let root = StateRoot::new(first.id());
        let error = root.add_file("foreign.txt", object).expect_err("must reject");

        assert!(matches!(
            error,
            StateRootError::StoreMismatch(StoreMismatch {
                expected: ObjectStoreId::new(1),
                actual: ObjectStoreId::new(2),
            })
        ));
    }

    #[test]
    fn state_root_rejects_duplicate_names() {
        let mut store = store();
        let object = store.intern(b"data");
        let root = StateRoot::new(store.id())
            .add_file("a.txt", object)
            .expect("first entry");

        let error = root.add_file("a.txt", object).expect_err("must reject");

        assert_eq!(error, StateRootError::DuplicateName("a.txt".to_owned()));
    }

    #[test]
    fn state_root_rejects_unknown_names_on_replace() {
        let mut store = store();
        let object = store.intern(b"data");
        let root = StateRoot::new(store.id());

        let error = root
            .replace_object("missing.txt", object)
            .expect_err("must reject");

        assert_eq!(
            error,
            StateRootError::MissingName("missing.txt".to_owned())
        );
    }

    #[test]
    fn from_entries_validates_all_store_boundaries() {
        let mut first = ObjectStore::new(ObjectStoreId::new(1));
        let mut second = ObjectStore::new(ObjectStoreId::new(2));
        let first_object = first.intern(b"first");
        let second_object = second.intern(b"second");

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
            StoreMismatch {
                expected: first.id(),
                actual: second.id(),
            }
        );
    }
}
