#![forbid(unsafe_code)]

mod codec;
mod error;
mod id;
mod state;
mod store;

pub use codec::{Codec, ZlibCodec};
pub use error::DarcError;
pub use id::{ObjectId, ObjectStoreId};
pub use state::{FileEntry, StateRoot};
pub use store::{ObjectStore, StoredObject};

/// Current public crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        Codec, DarcError, ObjectId, ObjectStore, ObjectStoreId, StateRoot, VERSION, ZlibCodec,
    };

    #[test]
    fn version_is_non_empty() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn identical_files_share_physical_objects() {
        let mut store = ObjectStore::with_id(
            ObjectStoreId::from_bytes([1; 16]),
            8,
            ZlibCodec::default(),
        );
        let payload = b"12345678".repeat(100);
        let mut files = BTreeMap::new();
        files.insert("one.bin".to_string(), payload.clone());
        files.insert("two.bin".to_string(), payload.clone());
        files.insert("three.bin".to_string(), payload);

        let root = store.make_root(&files).unwrap();
        let one = root.file("one.bin").unwrap();
        let two = root.file("two.bin").unwrap();
        let three = root.file("three.bin").unwrap();
        assert_eq!(one.blocks(), two.blocks());
        assert_eq!(two.blocks(), three.blocks());
        assert_eq!(store.physical_object_count(), 1);
    }

    #[test]
    fn cross_store_object_is_rejected() {
        let mut a = ObjectStore::with_id(
            ObjectStoreId::from_bytes([1; 16]),
            4,
            ZlibCodec::default(),
        );
        let mut b = ObjectStore::with_id(
            ObjectStoreId::from_bytes([2; 16]),
            4,
            ZlibCodec::default(),
        );
        let foreign = b.intern(b"x").unwrap();
        assert!(matches!(a.get(&foreign), Err(DarcError::WrongStore)));
    }

    #[test]
    fn replacement_reuses_unchanged_references() {
        let mut store = ObjectStore::with_id(
            ObjectStoreId::from_bytes([3; 16]),
            4,
            ZlibCodec::default(),
        );
        let mut files = BTreeMap::new();
        files.insert("a.txt".to_string(), b"AAAA".to_vec());
        files.insert("b.txt".to_string(), b"BBBB".to_vec());
        let root1 = store.make_root(&files).unwrap();
        let a_before = root1.file("a.txt").unwrap().blocks().to_vec();
        let root2 = store.replace_file(&root1, "b.txt", b"CCCC").unwrap();
        let a_after = root2.file("a.txt").unwrap().blocks().to_vec();

        assert_eq!(a_before, a_after);
        assert_eq!(
            store.materialize(&root2).unwrap().get("b.txt").unwrap(),
            b"CCCC"
        );
    }

    #[test]
    fn corrupted_payload_fails_integrity() {
        let mut store = ObjectStore::with_id(
            ObjectStoreId::from_bytes([4; 16]),
            64,
            ZlibCodec::default(),
        );
        let object_id = store.intern(b"important").unwrap();
        store.corrupt_for_test(&object_id).unwrap();
        assert!(matches!(
            store.get(&object_id),
            Err(DarcError::IntegrityFailure)
        ));
    }

    #[test]
    fn foreign_root_is_rejected() {
        let a = ObjectStore::with_id(
            ObjectStoreId::from_bytes([5; 16]),
            4,
            ZlibCodec::default(),
        );
        let foreign =
            StateRoot::new_for_test(ObjectStoreId::from_bytes([6; 16]), Vec::new());
        assert!(matches!(
            a.verify_root(&foreign),
            Err(DarcError::WrongStore)
        ));
    }

    #[test]
    fn codec_round_trip() {
        let codec = ZlibCodec::default();
        let input = b"repeat-repeat-repeat".repeat(100);
        let encoded = codec.encode(&input).unwrap();
        let decoded = codec.decode(&encoded, input.len()).unwrap();
        assert_eq!(decoded, input);
        assert!(encoded.len() < input.len());
    }

    #[test]
    fn object_id_is_store_scoped() {
        let digest = [9; 32];
        let a = ObjectId::from_parts(ObjectStoreId::from_bytes([1; 16]), digest);
        let b = ObjectId::from_parts(ObjectStoreId::from_bytes([2; 16]), digest);
        assert_ne!(a, b);
    }
}
