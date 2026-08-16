//! Filesystem-backed receipt object store.

use std::fmt;
use std::fs;
use std::path::PathBuf;

use super::error::ReceiptError;
use super::object_store::{ReceiptObjectStore, validate_object_key};

/// Object store rooted at a configured directory on the host filesystem.
pub struct FilesystemObjectStore {
    root: PathBuf,
}

impl FilesystemObjectStore {
    pub fn new(root: PathBuf) -> Result<Self, ReceiptError> {
        if root.as_os_str().is_empty() {
            return Err(ReceiptError::validation(
                "storage directory must not be empty",
            ));
        }
        fs::create_dir_all(&root).map_err(|_| {
            ReceiptError::dependency("failed to initialize filesystem object store root")
        })?;
        Ok(Self { root })
    }

    fn resolve_path(&self, key: &str) -> Result<PathBuf, ReceiptError> {
        validate_object_key(key)?;
        let relative = key_to_relative_path(key)?;
        let path = self.root.join(relative);
        if !path.starts_with(&self.root) {
            return Err(ReceiptError::validation("object key escapes storage root"));
        }
        Ok(path)
    }
}

impl fmt::Debug for FilesystemObjectStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FilesystemObjectStore")
            .field("root", &"[REDACTED]")
            .finish()
    }
}

impl ReceiptObjectStore for FilesystemObjectStore {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ReceiptError> {
        let path = self.resolve_path(key)?;
        let parent = path
            .parent()
            .ok_or_else(|| ReceiptError::validation("object key must include a path component"))?;
        fs::create_dir_all(parent)
            .map_err(|_| ReceiptError::dependency("failed to create object parent directory"))?;
        let temp_name = format!(".{}.tmp", uuid::Uuid::new_v4());
        let temp_path = parent.join(temp_name);
        fs::write(&temp_path, bytes)
            .map_err(|_| ReceiptError::dependency("failed to write object bytes"))?;
        match fs::rename(&temp_path, &path) {
            Ok(()) => Ok(()),
            Err(_) => {
                let _ = fs::remove_file(&temp_path);
                Err(ReceiptError::dependency("failed to finalize object write"))
            }
        }
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ReceiptError> {
        let path = self.resolve_path(key)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(ReceiptError::dependency("failed to read object")),
        }
    }

    fn delete(&self, key: &str) -> Result<(), ReceiptError> {
        let path = self.resolve_path(key)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ReceiptError::dependency("failed to delete object")),
        }
    }
}

fn key_to_relative_path(key: &str) -> Result<PathBuf, ReceiptError> {
    let mut path = PathBuf::new();
    for component in key.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(ReceiptError::validation("object key is invalid"));
        }
        if component.contains('\\') || component.contains('\0') {
            return Err(ReceiptError::validation("object key is invalid"));
        }
        path.push(component);
    }
    if path.as_os_str().is_empty() {
        return Err(ReceiptError::validation("object key must not be empty"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_omits_root_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FilesystemObjectStore::new(dir.path().to_path_buf()).expect("store");
        let debug = format!("{store:?}");
        assert!(debug.contains("FilesystemObjectStore"));
        assert!(!debug.contains(dir.path().to_str().expect("path")));
    }
}
