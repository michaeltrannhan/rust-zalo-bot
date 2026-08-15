//! In-memory receipt object store for Milestone 4.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use super::error::ReceiptError;

/// Narrow object-store seam for receipt originals.
pub trait ReceiptObjectStore: Send + Sync {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ReceiptError>;
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ReceiptError>;
    fn delete(&self, key: &str) -> Result<(), ReceiptError>;
}

/// Process-local object store backed by a map.
#[derive(Default)]
pub struct InMemoryObjectStore {
    objects: RwLock<HashMap<String, Vec<u8>>>,
}

impl InMemoryObjectStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn stored_object_count(&self) -> usize {
        self.objects.read().map(|guard| guard.len()).unwrap_or(0)
    }
}

impl fmt::Debug for InMemoryObjectStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InMemoryObjectStore")
            .field("object_count", &self.stored_object_count())
            .finish()
    }
}

impl ReceiptObjectStore for InMemoryObjectStore {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ReceiptError> {
        if key.trim().is_empty() {
            return Err(ReceiptError::validation("object key must not be empty"));
        }
        let mut guard = self
            .objects
            .write()
            .map_err(|_| ReceiptError::dependency("object store lock poisoned"))?;
        guard.insert(key.to_string(), bytes.to_vec());
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ReceiptError> {
        let guard = self
            .objects
            .read()
            .map_err(|_| ReceiptError::dependency("object store lock poisoned"))?;
        Ok(guard.get(key).cloned())
    }

    fn delete(&self, key: &str) -> Result<(), ReceiptError> {
        let mut guard = self
            .objects
            .write()
            .map_err(|_| ReceiptError::dependency("object store lock poisoned"))?;
        guard.remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_omits_keys_and_bytes() {
        let store = InMemoryObjectStore::default();
        store
            .put("receipts/secret-key/abc", b"raw-bytes-secret")
            .expect("put");
        let debug = format!("{store:?}");
        assert!(debug.contains("object_count"));
        assert!(!debug.contains("secret-key"));
        assert!(!debug.contains("raw-bytes-secret"));
        assert!(!debug.contains("abc"));
    }
}
