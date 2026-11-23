//! Durable completion notifications. Delivery is at least once: a remote acknowledgement
//! followed by a local crash can produce a duplicate. Approval requests are never queued.
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Queue {
    dir: PathBuf,
    url: String,
    client: reqwest::Client,
    started: AtomicBool,
    wake: tokio::sync::Notify,
}

impl Queue {
    pub(crate) fn new(home: &Path, url: &str) -> Arc<Self> {
        let fingerprint = url.bytes().fold(0xcbf29ce484222325u64, |hash, b| {
            (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
        });
        Arc::new(Self {
            dir: home
                .join("outbox")
                .join("notifications")
                .join(format!("{fingerprint:016x}")),
            url: url.to_string(),
            client: reqwest::Client::new(),
            started: AtomicBool::new(false),
            wake: tokio::sync::Notify::new(),
        })
    }

    pub(crate) fn target(&self) -> String {
        self.dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }

    /// Commit before returning to the caller, before any network request can begin.
    pub(crate) fn enqueue(self: &Arc<Self>, text: &str) -> std::io::Result<()> {
        self.store(None, text)
    }

    pub(crate) fn enqueue_once(self: &Arc<Self>, key: &str, text: &str) -> std::io::Result<()> {
        let id = format!("once-{}", jeeves_state::ledger::digest_of(key.as_bytes()));
        self.store(Some(&id), text)
    }

    fn store(self: &Arc<Self>, key: Option<&str>, text: &str) -> std::io::Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!(
            "{stamp}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let temporary = self.dir.join(format!("{id}.tmp"));
        let committed = self.dir.join(format!("{}.json", key.unwrap_or(&id)));
        let _lock = if key.is_some() {
            Some(self.lock()?)
        } else {
            None
        };
        if key.is_some()
            && (committed.try_exists()? || committed.with_extension("delivered").try_exists()?)
        {
            #[cfg(unix)]
            std::fs::File::open(&self.dir)?.sync_all()?;
            return Ok(());
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(serde_json::json!({"text": text}).to_string().as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &committed)?;
        #[cfg(unix)]
        std::fs::File::open(&self.dir)?.sync_all()?;
        self.start();
        self.wake.notify_one();
        Ok(())
    }

    fn lock(&self) -> std::io::Result<std::fs::File> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join(".admission.lock"))?;
        file.lock()?;
        Ok(file)
    }

    fn delivered(&self, path: &Path) -> std::io::Result<()> {
        if path
            .file_stem()
            .is_some_and(|s| s.to_string_lossy().starts_with("once-"))
        {
            let _guard = self.lock()?;
            std::fs::File::create(path.with_extension("delivered"))?.sync_all()?;
            std::fs::File::open(&self.dir)?.sync_all()?;
        }
        std::fs::remove_file(path)
    }

    pub(crate) fn start(self: &Arc<Self>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = Arc::downgrade(self);
        runtime.spawn(async move {
            loop {
                let Some(queue) = weak.upgrade() else {
                    break;
                };
                queue.flush().await;
                tokio::select! {
                    _ = queue.wake.notified() => {},
                    _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {},
                }
                if Arc::strong_count(&queue) == 1 {
                    break;
                }
            }
        });
    }

    async fn flush(&self) {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                eprintln!("(jeeves) notification outbox unavailable: {e}");
                return;
            }
        };
        for entry in entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .take(32)
        {
            let path = entry.path();
            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            if file.try_lock().is_err() {
                continue;
            }
            let delivered = match path.with_extension("delivered").try_exists() {
                Ok(found) => found,
                Err(error) => {
                    eprintln!("(jeeves) notification delivery marker unavailable: {error}");
                    continue;
                }
            };
            if delivered {
                if let Err(error) = std::fs::remove_file(&path) {
                    eprintln!("(jeeves) delivered notification cleanup failed: {error}");
                }
                continue;
            }
            let payload = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
            let Some(payload) = payload.filter(|p| p.get("text").is_some_and(|t| t.is_string()))
            else {
                eprintln!(
                    "(jeeves) malformed pending notification retained: {}",
                    path.display()
                );
                continue;
            };
            match self
                .client
                .post(&self.url)
                .json(&payload)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    if let Err(e) = self.delivered(&path) {
                        eprintln!(
                            "(jeeves) notification delivered but receipt could not be cleared: {e}"
                        );
                    }
                }
                Ok(response) => {
                    eprintln!(
                        "(jeeves) notification retained for retry: HTTP {}",
                        response.status()
                    );
                    break;
                }
                Err(_) => {
                    eprintln!("(jeeves) notification retained for retry: transport failure");
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_after_enqueue_or_after_delivery_preserves_one_logical_notification() {
        let home = tempfile::tempdir().unwrap();
        let url = "http://127.0.0.1:9/fixture";
        let original = Queue::new(home.path(), url);
        original.enqueue_once("task:receipt:1", "finished").unwrap();
        let reopened = Queue::new(home.path(), url);
        reopened.enqueue_once("task:receipt:1", "finished").unwrap();
        let pending = || {
            std::fs::read_dir(&reopened.dir)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|s| s == "json"))
                .collect::<Vec<_>>()
        };
        assert_eq!(pending().len(), 1);
        reopened.delivered(&pending()[0]).unwrap();
        Queue::new(home.path(), url)
            .enqueue_once("task:receipt:1", "finished")
            .unwrap();
        assert!(pending().is_empty());
        reopened.enqueue_once("task:receipt:2", "finished").unwrap();
        assert_eq!(pending().len(), 1);
    }

    #[test]
    fn notification_is_committed_before_return_without_storing_credentials() {
        let home = tempfile::tempdir().unwrap();
        let queue = Queue::new(home.path(), "https://example.invalid/secret-webhook");
        queue.enqueue("job finished").unwrap();
        let files: Vec<_> = std::fs::read_dir(&queue.dir).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        let body = std::fs::read_to_string(files[0].path()).unwrap();
        assert!(body.contains("job finished"));
        assert!(!body.contains("secret-webhook"));
        assert_eq!(files[0].path().extension().unwrap(), "json");
        let different = Queue::new(home.path(), "https://example.invalid/other-target");
        assert_ne!(queue.dir, different.dir);
    }

    #[tokio::test]
    async fn failed_delivery_survives_recreation_and_is_cleared_only_on_ack() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let remote = tokio::spawn(async move {
            for status in ["500 Internal Server Error", "200 OK"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                let n = stream.read(&mut bytes).await.unwrap();
                assert!(n > 0, "the mock server should receive an HTTP request");
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let home = tempfile::tempdir().unwrap();
        let queue = Queue::new(home.path(), &url);
        let writer = queue.clone();
        std::thread::spawn(move || writer.enqueue("finished").unwrap())
            .join()
            .unwrap();
        queue.flush().await;
        assert_eq!(std::fs::read_dir(&queue.dir).unwrap().count(), 1);
        drop(queue);
        let restored = Queue::new(home.path(), &url);
        restored.flush().await;
        assert_eq!(std::fs::read_dir(&restored.dir).unwrap().count(), 0);
        remote.await.unwrap();
    }
}
