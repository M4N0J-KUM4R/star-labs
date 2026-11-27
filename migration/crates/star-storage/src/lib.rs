//! star-storage — the byte store behind the star-lms attachments.
//!
//! Canvas keeps the bytes of an attachment in an ActiveStorage-style store
//! (`app/models/attachment.rb`, `app/models/attachments/storage.rb`) and
//! addresses them with a key derived from the attachment id. This crate is
//! the port of that: a [`ObjectStore`] trait with a local-filesystem
//! implementation and an S3-compatible one, both keyed the way Canvas keys
//! them (see [`key`]).
//!
//! Configuration comes from the environment:
//!
//! | variable | meaning | default |
//! |---|---|---|
//! | `STAR_STORAGE_BACKEND` | `local` or `s3` | `local` |
//! | `STAR_STORAGE_DIR` | root of the local backend | `tmp/files` (Canvas' `path_prefix`) |
//! | `STAR_S3_BUCKET` | bucket name | — |
//! | `STAR_S3_REGION` | region | `us-east-1` |
//! | `STAR_S3_ENDPOINT` | endpoint of an S3-compatible service | AWS |
//! | `STAR_S3_ACCESS_KEY_ID` | access key | — |
//! | `STAR_S3_SECRET_ACCESS_KEY` | secret key | — |
//! | `STAR_S3_NAMESPACE` | Canvas' account namespace for S3 keys | none |

pub mod key;
pub mod local;
pub mod s3;

use std::sync::{Arc, OnceLock, RwLock};

use md5::Digest;

pub use key::{canvas_filesystem_key, canvas_s3_key, sanitize_filename};
pub use local::LocalFileStore;
pub use s3::{S3Config, S3Store};

/// The backend that failed.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The object is not in the store.
    #[error("object not found: {0}")]
    NotFound(String),
    /// The local filesystem refused the operation.
    #[error("io error: {0}")]
    Io(String),
    /// The object store answered with an error.
    #[error("http error: {0}")]
    Http(String),
    /// The store is not configured well enough to run.
    #[error("storage configuration error: {0}")]
    Config(String),
}

/// What a successful write produced.
#[derive(Debug, Clone)]
pub struct StoredObject {
    /// The key the bytes live under.
    pub key: String,
    /// The size of the stored bytes.
    pub size: u64,
    /// The content digest — MD5 hex for the local backend, the S3 ETag for S3.
    pub digest: Option<String>,
}

/// A boxed future, so the trait stays usable as a trait object.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// The byte store behind the attachments API.
///
/// Keys are opaque strings; the store decides what they look like for the
/// objects it holds ([`ObjectStore::attachment_key`]).
pub trait ObjectStore: Send + Sync {
    /// The Canvas-compatible key for an attachment's bytes.
    fn attachment_key(&self, attachment_id: i64, filename: &str) -> String;

    /// Store `bytes` under `key`.
    fn put<'a>(
        &'a self,
        key: &'a str,
        bytes: &'a [u8],
        content_type: Option<&'a str>,
    ) -> BoxFuture<'a, Result<StoredObject, StorageError>>;

    /// Read the bytes stored under `key`.
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Vec<u8>, StorageError>>;

    /// Remove the bytes stored under `key`. Removing a missing object is not
    /// an error.
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<(), StorageError>>;

    /// Whether an object is stored under `key`.
    fn exists<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<bool, StorageError>>;

    /// The size of the object stored under `key`.
    fn size<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<u64, StorageError>>;
}

/// The MD5 hex digest of `bytes`.
///
/// Canvas stores the MD5 of the file in `attachments.md5`
/// (`app/models/attachment.rb:1097-1109`), so a file uploaded through the
/// port and the same file uploaded through Canvas carry the same digest.
pub fn md5_hex(bytes: &[u8]) -> String {
    to_hex(&md5::Md5::digest(bytes))
}

/// Lower-case hex encoding.
pub fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Which backend the process runs.
#[derive(Debug, Clone)]
pub enum Backend {
    /// A directory on disk.
    Local { dir: std::path::PathBuf },
    /// An S3-compatible object store.
    S3(S3Config),
}

impl Backend {
    /// Read the backend out of the environment.
    pub fn from_env() -> Result<Self, StorageError> {
        let backend = std::env::var("STAR_STORAGE_BACKEND")
            .unwrap_or_else(|_| "local".to_string())
            .trim()
            .to_ascii_lowercase();
        match backend.as_str() {
            "local" | "" => Ok(Backend::Local {
                dir: std::env::var("STAR_STORAGE_DIR")
                    .unwrap_or_else(|_| "tmp/files".to_string())
                    .into(),
            }),
            "s3" => Ok(Backend::S3(S3Config::from_env()?)),
            other => Err(StorageError::Config(format!(
                "unknown STAR_STORAGE_BACKEND `{other}` (expected `local` or `s3`)"
            ))),
        }
    }

    /// Build the store this backend describes.
    pub fn build(&self) -> Result<Arc<dyn ObjectStore>, StorageError> {
        match self {
            Backend::Local { dir } => Ok(Arc::new(LocalFileStore::new(dir.clone()))),
            Backend::S3(config) => Ok(Arc::new(S3Store::new(config.clone())?)),
        }
    }
}

static GLOBAL: OnceLock<RwLock<Option<Arc<dyn ObjectStore>>>> = OnceLock::new();

fn global_cell() -> &'static RwLock<Option<Arc<dyn ObjectStore>>> {
    GLOBAL.get_or_init(|| RwLock::new(None))
}

/// The process-wide store, built from the environment on first use.
///
/// The store is a process-wide singleton because the backend is a property of
/// the deployment, not of a request.
pub fn global_store() -> Result<Arc<dyn ObjectStore>, StorageError> {
    if let Some(store) = global_cell().read().ok().and_then(|cell| cell.clone()) {
        return Ok(store);
    }
    let store = Backend::from_env()?.build()?;
    let mut cell = global_cell()
        .write()
        .map_err(|_| StorageError::Config("the global storage lock was poisoned".to_string()))?;
    if cell.is_none() {
        *cell = Some(store.clone());
    }
    Ok(cell.clone().unwrap_or(store))
}

/// Install a store, replacing whatever the environment produced.
pub fn set_global_store(store: Arc<dyn ObjectStore>) {
    if let Ok(mut cell) = global_cell().write() {
        *cell = Some(store);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_canvas_digest() {
        // Canvas: Digest::MD5.hexdigest("hello")
        assert_eq!(md5_hex(b"hello"), "5d41402abc4b2a76b9719d911017c592");
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    }
}
