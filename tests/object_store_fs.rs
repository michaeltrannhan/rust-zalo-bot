//! Filesystem object store integration tests.

use std::sync::Arc;

use tempfile::TempDir;
use uuid::Uuid;
use zl_expense::error::ErrorClass;
use zl_expense::receipt::{FilesystemObjectStore, ReceiptObjectStore, object_key, validate_image};

fn sample_key() -> String {
    let account_id = Uuid::new_v4();
    let submission_id = Uuid::new_v4();
    let image = validate_image(
        include_bytes!("../src/receipt/testdata/tiny.png"),
        "image/png",
    )
    .expect("validate image");
    object_key(account_id, submission_id, &image.content_sha256)
}

fn open_store(dir: &TempDir) -> Arc<FilesystemObjectStore> {
    Arc::new(FilesystemObjectStore::new(dir.path().to_path_buf()).expect("open filesystem store"))
}

#[test]
fn put_get_delete_round_trip() {
    let dir = TempDir::new().expect("tempdir");
    let store = open_store(&dir);
    let key = sample_key();
    let bytes = b"receipt-bytes";

    store.put(&key, bytes).expect("put");
    let loaded = store.get(&key).expect("get").expect("object");
    assert_eq!(loaded, bytes);
    store.delete(&key).expect("delete");
    assert!(store.get(&key).expect("get after delete").is_none());
}

#[test]
fn missing_get_returns_none() {
    let dir = TempDir::new().expect("tempdir");
    let store = open_store(&dir);
    assert!(
        store
            .get("receipts/missing/submission/hash")
            .expect("get")
            .is_none()
    );
}

#[test]
fn missing_delete_is_ok() {
    let dir = TempDir::new().expect("tempdir");
    let store = open_store(&dir);
    store
        .delete("receipts/missing/submission/hash")
        .expect("delete missing");
}

#[test]
fn rejects_path_traversal_keys() {
    let dir = TempDir::new().expect("tempdir");
    let store = open_store(&dir);
    let error = store.put("receipts/../escape/object", b"x").unwrap_err();
    assert_eq!(error.class, ErrorClass::Validation);
}

#[test]
fn survives_store_restart_on_same_directory() {
    let dir = TempDir::new().expect("tempdir");
    let key = sample_key();
    let bytes = b"durable-payload";

    let first = open_store(&dir);
    first.put(&key, bytes).expect("put");

    let second = open_store(&dir);
    let loaded = second.get(&key).expect("get").expect("object");
    assert_eq!(loaded, bytes);
}
