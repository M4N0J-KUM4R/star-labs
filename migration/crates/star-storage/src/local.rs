//! The local-filesystem backend.
//!
//! Canvas' file-system backend keeps the bytes under `Rails.root` +
//! `path_prefix` (`config/file_store.yml` -> `tmp/files`) and addresses them
//! with the partitioned key described in [`crate::key`].

use std::fs;
use std::path::{Path, PathBuf};

use crate::key::canvas_filesystem_key;
use crate::{md5_hex, BoxFuture, ObjectStore, StorageError, StoredObject};

/// A byte store rooted at a directory on disk.
#[derive(Debug, Clone)]
pub struct LocalFileStore {
    root: PathBuf,
}

impl LocalFileStore {
    /// Root the store at `root`. The directory is created on the first write.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory the bytes live in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a key to a path inside the root, refusing keys that would
    /// escape it.
    fn path_for(&self, key: &str) -> Result<PathBuf, StorageError> {
        let mut path = self.root.clone();
        for component in key.split('/') {
            match component {
                "" | "." => continue,
                ".." => {
                    return Err(StorageError::Config(format!(
                        "storage key escapes the storage root: {key}"
                    )))
                }
                other => path.push(other),
            }
        }
        Ok(path)
    }

    /// Store `bytes` under `key`.
    pub fn put_bytes(
        &self,
        key: &str,
        bytes: &[u8],
        _content_type: Option<&str>,
    ) -> Result<StoredObject, StorageError> {
        let path = self.path_for(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| StorageError::Io(format!("create {}: {e}", parent.display())))?;
        }
        fs::write(&path, bytes)
            .map_err(|e| StorageError::Io(format!("write {}: {e}", path.display())))?;
        Ok(StoredObject {
            key: key.to_string(),
            size: bytes.len() as u64,
            digest: Some(md5_hex(bytes)),
        })
    }

    /// Read the bytes stored under `key`.
    pub fn get_bytes(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        let path = self.path_for(key)?;
        fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => StorageError::NotFound(key.to_string()),
            _ => StorageError::Io(format!("read {}: {e}", path.display())),
        })
    }

    /// Remove the bytes stored under `key`.
    pub fn delete_bytes(&self, key: &str) -> Result<(), StorageError> {
        let path = self.path_for(key)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StorageError::Io(format!("remove {}: {e}", path.display()))),
        }
    }

    /// Whether an object is stored under `key`.
    pub fn has_bytes(&self, key: &str) -> Result<bool, StorageError> {
        let path = self.path_for(key)?;
        Ok(path.is_file())
    }

    /// The size of the object stored under `key`.
    pub fn size_of(&self, key: &str) -> Result<u64, StorageError> {
        let path = self.path_for(key)?;
        fs::metadata(&path)
            .map(|m| m.len())
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => StorageError::NotFound(key.to_string()),
                _ => StorageError::Io(format!("stat {}: {e}", path.display())),
            })
    }
}

impl ObjectStore for LocalFileStore {
    fn attachment_key(&self, attachment_id: i64, filename: &str) -> String {
        canvas_filesystem_key(attachment_id, filename)
    }

    fn put<'a>(
        &'a self,
        key: &'a str,
        bytes: &'a [u8],
        content_type: Option<&'a str>,
    ) -> BoxFuture<'a, Result<StoredObject, StorageError>> {
        Box::pin(async move { self.put_bytes(key, bytes, content_type) })
    }

    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Vec<u8>, StorageError>> {
        Box::pin(async move { self.get_bytes(key) })
    }

    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(async move { self.delete_bytes(key) })
    }

    fn exists<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<bool, StorageError>> {
        Box::pin(async move { self.has_bytes(key) })
    }

    fn size<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<u64, StorageError>> {
        Box::pin(async move { self.size_of(key) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_bytes() {
        let dir = std::env::temp_dir().join(format!("star-storage-test-{}", std::process::id()));
        let store = LocalFileStore::new(&dir);
        let key = store.attachment_key(569, "report.pdf");
        assert_eq!(key, "0000/0569/report.pdf");

        let put = store
            .put_bytes(&key, b"hello", Some("application/pdf"))
            .unwrap();
        assert_eq!(put.size, 5);
        assert_eq!(
            put.digest.as_deref(),
            Some("5d41402abc4b2a76b9719d911017c592")
        );

        assert!(store.has_bytes(&key).unwrap());
        assert_eq!(store.size_of(&key).unwrap(), 5);
        assert_eq!(store.get_bytes(&key).unwrap(), b"hello".to_vec());

        store.delete_bytes(&key).unwrap();
        assert!(!store.has_bytes(&key).unwrap());
        assert!(matches!(
            store.get_bytes(&key),
            Err(StorageError::NotFound(_))
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_keys_that_escape_the_root() {
        let store = LocalFileStore::new("/tmp/star-storage-root");
        assert!(matches!(
            store.path_for("../escape"),
            Err(StorageError::Config(_))
        ));
    }
}
