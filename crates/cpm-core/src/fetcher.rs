//! Asset fetcher — downloads git blobs, npm tarballs, and release binaries.
//!
//! # Design
//! - All HTTP via `reqwest` async — never blocking.
//! - Blobs are cached under `~/.cache/cpm/objects/<sha>/`.
//! - npm tarballs are verified against the registry `integrity` field before
//!   caching.
//! - GitHub release binaries are streamed to
//!   `~/.cache/cpm/bins/<name>-<version>-<arch>` and `chmod +x`-ed on Unix.
//! - Docker images are **not** pulled at install time — only recorded in lock.
//! - Respects the `CPM_CACHE_DIR` env var (default `~/.cache/cpm`).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::{debug, info};

use crate::CpmError;

/// Receives lifecycle events for a streamed download.
pub trait DownloadProgress: Send + Sync {
    /// Start tracking a download for `url`.
    ///
    /// `total_bytes` is provided when the response includes a content length.
    fn begin(&self, url: &str, total_bytes: Option<u64>) -> Box<dyn DownloadProgressHandle>;
}

/// Mutable handle for one in-flight download.
pub trait DownloadProgressHandle: Send {
    /// Record that `delta` additional bytes have been received.
    fn advance(&mut self, delta: u64);

    /// Mark the download as completed successfully.
    fn finish(&mut self);

    /// Mark the download as failed.
    fn fail(&mut self);
}

/// Return the cache root directory.
///
/// Uses `CPM_CACHE_DIR` if set, otherwise falls back to the platform cache
/// directory (`~/.cache/cpm` on Linux, `~/Library/Caches/cpm` on macOS,
/// `%LOCALAPPDATA%\cpm` on Windows).
pub fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CPM_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp/.cpm-cache"))
        .join("cpm")
}

/// Compute a `"sha256:<hex>"` digest for arbitrary bytes.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Compute a `"sha256:<hex>"` digest for a file on disk.
///
/// # Errors
/// Returns [`CpmError::Io`] if the file cannot be read.
pub fn sha256_file(path: &Path) -> Result<String, CpmError> {
    let data = std::fs::read(path)?;
    Ok(sha256_hex(&data))
}

/// Compute the combined content hash for a set of installed files.
///
/// Files are sorted lexicographically by their path string before hashing so
/// the result is deterministic regardless of iteration order.
///
/// # Errors
/// Returns [`CpmError::Io`] if any file cannot be read.
pub fn hash_installed_files(files: &[PathBuf]) -> Result<String, CpmError> {
    let mut sorted = files.to_vec();
    sorted.sort();

    let mut hasher = Sha256::new();
    for path in &sorted {
        let file_hash = sha256_file(path)?;
        hasher.update(file_hash.as_bytes());
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Fetch the bytes at `url` using the provided HTTP client.
///
/// Passes the token as a `Bearer` authorisation header when present.
///
/// # Errors
/// - [`CpmError::Network`] on request failure.
/// - [`CpmError::AuthRequired`] on HTTP 401/403.
pub async fn fetch_bytes(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    progress: Option<&dyn DownloadProgress>,
) -> Result<Vec<u8>, CpmError> {
    debug!("fetching {url}");
    let mut req = client.get(url);
    if let Some(tok) = token {
        req = req.bearer_auth(tok);
    }
    let resp = req.send().await?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(CpmError::AuthRequired {
            url: url.to_owned(),
        });
    }
    let mut resp = resp.error_for_status()?;
    let total_bytes = resp.content_length();
    let mut progress = progress.map(|observer| observer.begin(url, total_bytes));
    let capacity = total_bytes
        .and_then(|len| usize::try_from(len).ok())
        .unwrap_or(0);
    let mut bytes = Vec::with_capacity(capacity);
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if let Some(handle) = progress.as_mut() {
                    handle.advance(chunk.len() as u64);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(err) => {
                if let Some(handle) = progress.as_mut() {
                    handle.fail();
                }
                return Err(err.into());
            }
        }
    }
    if let Some(handle) = progress.as_mut() {
        handle.finish();
    }
    info!("fetched {} bytes from {url}", bytes.len());
    Ok(bytes)
}

/// Write `data` to `dest` atomically (write to `<dest>.tmp`, then rename).
///
/// On Windows, `std::fs::rename` fails if the destination already exists, so
/// we remove it first. This makes the operation non-atomic on Windows, but it
/// prevents errors when overwriting existing files during installs/updates.
///
/// # Errors
/// Returns [`CpmError::Io`] on any filesystem error.
pub fn atomic_write(dest: &Path, data: &[u8]) -> Result<(), CpmError> {
    let tmp = dest.with_extension("tmp");
    if let Some(parent) = tmp.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tmp, data)?;
    // On Windows, rename fails when the destination already exists.
    // Remove the destination file first to allow the rename to succeed.
    #[cfg(windows)]
    if dest.exists() {
        std::fs::remove_file(dest)?;
    }
    std::fs::rename(&tmp, dest)?;
    debug!("wrote {} bytes to {}", data.len(), dest.display());
    Ok(())
}

/// Make a file executable on Unix systems (no-op on other platforms).
#[cfg(unix)]
pub fn make_executable(path: &Path) -> Result<(), CpmError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o111);
    std::fs::set_permissions(path, perms)?;
    debug!("chmod +x {}", path.display());
    Ok(())
}

/// No-op on non-Unix platforms.
#[cfg(not(unix))]
pub fn make_executable(_path: &Path) -> Result<(), CpmError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    /// Serialises all tests that mutate `CPM_CACHE_DIR` to prevent flaky
    /// failures when the Rust test runner executes them in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn sha256_hex_known_value() {
        // echo -n "hello" | sha256sum => 2cf24dba...
        let digest = sha256_hex(b"hello");
        assert!(digest.starts_with("sha256:"));
        assert_eq!(
            digest,
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn atomic_write_creates_file() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("test.txt");
        atomic_write(&path, b"hello").expect("write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "hello");
    }

    #[test]
    fn atomic_write_overwrites_existing_file() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("overwrite.txt");
        atomic_write(&path, b"first").expect("first write");
        atomic_write(&path, b"second").expect("overwrite");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "second");
    }

    #[test]
    fn hash_installed_files_order_independent() {
        let dir = TempDir::new().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"aaa").expect("write a");
        std::fs::write(&b, b"bbb").expect("write b");

        let hash1 = hash_installed_files(&[a.clone(), b.clone()]).expect("hash1");
        let hash2 = hash_installed_files(&[b, a]).expect("hash2");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn hash_installed_files_matches_canonical_file_hash_aggregate() {
        let dir = TempDir::new().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"aaa").expect("write a");
        std::fs::write(&b, b"bbb").expect("write b");

        let expected =
            sha256_hex(format!("{}{}", sha256_hex(b"aaa"), sha256_hex(b"bbb")).as_bytes());

        let actual = hash_installed_files(&[a, b]).expect("hash");
        assert_eq!(actual, expected);
    }

    #[test]
    fn cache_dir_uses_env_var() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        // SAFETY: test is serialised by ENV_LOCK.
        unsafe {
            std::env::set_var("CPM_CACHE_DIR", "/tmp/my-cpm-cache");
        }
        let dir = cache_dir();
        unsafe {
            std::env::remove_var("CPM_CACHE_DIR");
        }
        assert_eq!(dir, PathBuf::from("/tmp/my-cpm-cache"));
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum ProgressEvent {
        Started {
            url: String,
            total_bytes: Option<u64>,
        },
        Advanced(u64),
        Finished,
        Failed,
    }

    #[derive(Debug, Default)]
    struct RecordingProgress {
        events: Arc<Mutex<Vec<ProgressEvent>>>,
    }

    impl RecordingProgress {
        fn snapshot(&self) -> Vec<ProgressEvent> {
            self.events.lock().expect("events lock").clone()
        }
    }

    struct RecordingHandle {
        events: Arc<Mutex<Vec<ProgressEvent>>>,
    }

    impl DownloadProgress for RecordingProgress {
        fn begin(&self, url: &str, total_bytes: Option<u64>) -> Box<dyn DownloadProgressHandle> {
            self.events
                .lock()
                .expect("events lock")
                .push(ProgressEvent::Started {
                    url: url.to_owned(),
                    total_bytes,
                });
            Box::new(RecordingHandle {
                events: Arc::clone(&self.events),
            })
        }
    }

    impl DownloadProgressHandle for RecordingHandle {
        fn advance(&mut self, delta: u64) {
            self.events
                .lock()
                .expect("events lock")
                .push(ProgressEvent::Advanced(delta));
        }

        fn finish(&mut self) {
            self.events
                .lock()
                .expect("events lock")
                .push(ProgressEvent::Finished);
        }

        fn fail(&mut self) {
            self.events
                .lock()
                .expect("events lock")
                .push(ProgressEvent::Failed);
        }
    }

    #[tokio::test]
    async fn fetch_bytes_reports_progress_events() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/asset"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", "11")
                    .set_body_bytes("hello world"),
            )
            .mount(&server)
            .await;

        let progress = RecordingProgress::default();
        let url = format!("{}/asset", server.uri());
        let bytes = fetch_bytes(&reqwest::Client::new(), &url, None, Some(&progress))
            .await
            .expect("fetch");

        assert_eq!(bytes, b"hello world");
        let events = progress.snapshot();
        assert_eq!(
            events.first(),
            Some(&ProgressEvent::Started {
                url: url.clone(),
                total_bytes: Some(11),
            })
        );
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    ProgressEvent::Advanced(delta) => Some(*delta),
                    _ => None,
                })
                .sum::<u64>(),
            11
        );
        assert_eq!(events.last(), Some(&ProgressEvent::Finished));
        assert!(!events.contains(&ProgressEvent::Failed));
    }
}
