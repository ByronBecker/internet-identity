use super::*;
use hashtree::Hash;
use sha2::{Digest, Sha256};

fn hash_bytes(value: impl AsRef<[u8]>) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(value.as_ref());
    hasher.finalize().into()
}

fn seed(x: u64) -> Hash {
    hash_bytes(x.to_be_bytes())
}

fn message(x: u64) -> Hash {
    hash_bytes(x.to_le_bytes())
}

#[test]
fn test_signature_lookup() {
    let mut map = SignatureMap::default();
    map.put(seed(1), message(1), 10);
    assert_eq!(
        map.witness(seed(1), message(1))
            .expect("failed to get a witness")
            .reconstruct(),
        map.root_hash()
    );
    assert!(map.witness(seed(1), message(2)).is_none());
    assert!(map.witness(seed(2), message(1)).is_none());

    map.delete(seed(1), message(1));
    assert!(map.witness(seed(1), message(1)).is_none());
}

#[test]
fn test_signature_expiration() {
    let mut map = SignatureMap::default();

    map.put(seed(1), message(1), 10);
    map.put(seed(1), message(2), 20);
    map.put(seed(2), message(1), 15);
    map.put(seed(2), message(2), 25);

    assert_eq!(2, map.prune_expired(/*time now*/ 19, /*max_to_prune*/ 10));
    assert!(map.witness(seed(1), message(1)).is_none());
    assert!(map.witness(seed(2), message(1)).is_none());

    assert!(map.witness(seed(1), message(2)).is_some());
    assert!(map.witness(seed(2), message(2)).is_some());
}

#[test]
fn test_signature_expiration_limit() {
    let mut map = SignatureMap::default();

    for i in 0..10 {
        map.put(seed(i), message(i), 10 * i);
    }

    assert_eq!(5, map.prune_expired(/*time now*/ 100, /*max_to_prune*/ 5));

    for i in 0..5 {
        assert!(map.witness(seed(i), message(i)).is_none());
    }
    for i in 5..10 {
        assert!(map.witness(seed(i), message(i)).is_some());
    }
}
