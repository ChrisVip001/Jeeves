//! One owned worker, bounded frames, independent control requests and no replay.
use anyhow::{anyhow, ensure, Result};
use jeeves_protocol::{BrowserWorkerCommand, BrowserWorkerRequest, BrowserWorkerResponse};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{mpsc, oneshot, watch},
};
type Pending = Arc<Mutex<HashMap<i32, oneshot::Sender<Result<BrowserWorkerResponse>>>>>;
pub(super) struct Worker {
    tx: mpsc::Sender<BrowserWorkerRequest>,
    stop: watch::Sender<()>,
    exited: watch::Receiver<bool>,
    remote: AtomicBool,
    pending: Pending,
    alive: Arc<AtomicBool>,
    next: AtomicI32,
}
impl Worker {
    pub async fn start(config: &jeeves_config::browser::BrowserRuntime) -> Result<Arc<Self>> {
        let root = config.root.canonicalize()?;
        let node = if root.join("node").is_file() {
            root.join("node")
        } else {
            root.join("node.exe")
        };
        let entry = root.join("worker/src/worker.ts");
        ensure!(
            node.is_file() && entry.is_file(),
            "Browser components are not installed"
        );
        let inherited: Vec<_> = std::env::vars()
            .filter(|(name, _)| {
                matches!(
                    name.as_str(),
                    "HOME"
                        | "TMPDIR"
                        | "TMP"
                        | "TEMP"
                        | "SystemRoot"
                        | "DISPLAY"
                        | "WAYLAND_DISPLAY"
                        | "XDG_RUNTIME_DIR"
                        | "DBUS_SESSION_BUS_ADDRESS"
                )
            })
            .collect();
        let scratch = tempfile::Builder::new()
            .prefix("jeeves-browser-")
            .tempdir()?;
        let mut child = Command::new(node)
            .arg(entry)
            .env_clear()
            .envs(inherited)
            .env("TMPDIR", scratch.path())
            .env("TEMP", scratch.path())
            .env("TMP", scratch.path())
            .env("PLAYWRIGHT_BROWSERS_PATH", root.join("browsers"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Worker input unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Worker output unavailable"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Worker diagnostics unavailable"))?;
        tokio::spawn(async move {
            let mut bytes = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut bytes).await {
                if n == 0 {
                    break;
                }
            }
        });
        let (tx, mut rx) = mpsc::channel::<BrowserWorkerRequest>(16);
        let (stop, mut reader_stop) = watch::channel(());
        let mut writer_stop = reader_stop.clone();
        let (exit_tx, exited) = watch::channel(false);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        tokio::spawn(async move {
            loop {
                let request = tokio::select! {
                    biased;
                    _ = writer_stop.changed() => break,
                    item = rx.recv() => match item { Some(item) => item, None => break },
                };
                let Ok(mut line) = serde_json::to_vec(&request) else {
                    break;
                };
                line.push(b'\n');
                tokio::select! {
                    biased;
                    _ = writer_stop.changed() => break,
                    result = stdin.write_all(&line) => if result.is_err() { break; }
                }
            }
            let _ = stdin.shutdown().await;
        });
        let requests = pending.clone();
        let running = alive.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut instance = None;
            loop {
                let mut line = Vec::new();
                let mut bounded = (&mut reader).take(1_500_001);
                tokio::select! {
                    biased;
                    _ = reader_stop.changed() => break,
                    result = bounded.read_until(b'\n', &mut line) => if !matches!(result, Ok(n) if n > 0) { break; }
                }
                if line.len() > 1_500_000 {
                    break;
                }
                let Ok(reply) = serde_json::from_slice::<BrowserWorkerResponse>(&line) else {
                    break;
                };
                let info = &reply.info;
                if info.schema_version != 1
                    || info.instance_id.is_empty()
                    || info.instance_id.len() > 80
                    || info.max_origins != 12
                    || info.max_frame_bytes != 800_000
                    || [
                        "observe", "navigate", "click", "fill", "scroll", "frame", "control",
                    ]
                    .iter()
                    .any(|name| {
                        !info
                            .capabilities
                            .iter()
                            .any(|capability| capability == name)
                    })
                    || instance.as_ref().is_some_and(|id| id != &info.instance_id)
                {
                    break;
                }
                instance = Some(info.instance_id.clone());
                if let Ok(mut pending) = requests.lock() {
                    if let Some(sender) = pending.remove(&reply.id) {
                        let _ = sender.send(Ok(reply));
                    }
                }
            }
            running.store(false, Ordering::Release);
            if let Ok(mut pending) = requests.lock() {
                for (_, sender) in pending.drain() {
                    let _ = sender.send(Err(anyhow!(
                        "Browser worker disconnected; effects are not replayed"
                    )));
                }
            }
            let exited = matches!(
                tokio::time::timeout(Duration::from_secs(1), child.wait()).await,
                Ok(Ok(_))
            );
            if !exited && child.kill().await.is_err() && !matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            drop(scratch);
            exit_tx.send_replace(true);
        });
        let worker = Arc::new(Self {
            tx,
            pending,
            alive,
            stop,
            exited,
            remote: AtomicBool::new(false),
            next: AtomicI32::new(1),
        });
        let hello = worker.request(BrowserWorkerCommand::Hello).await?;
        ensure!(hello.error.is_none(), "Browser worker handshake failed");
        worker.remote.store(
            ["humanInput", "downloads", "download"]
                .iter()
                .all(|cap| hello.info.capabilities.iter().any(|c| c == cap)),
            Ordering::Release,
        );
        Ok(worker)
    }
    pub fn remote_supported(&self) -> bool {
        self.remote.load(Ordering::Acquire)
    }
    pub fn is_closed(&self) -> bool {
        !self.alive.load(Ordering::Acquire)
    }
    pub fn close(&self) {
        self.alive.store(false, Ordering::Release);
        self.stop.send_replace(());
    }
    pub async fn wait_closed(&self) -> Result<()> {
        self.exited
            .clone()
            .wait_for(|exited| *exited)
            .await
            .map_err(|_| anyhow!("Browser worker termination could not be confirmed"))?;
        Ok(())
    }
    pub async fn request(&self, command: BrowserWorkerCommand) -> Result<BrowserWorkerResponse> {
        ensure!(
            self.alive.load(Ordering::Acquire),
            "Browser worker is closed"
        );
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| anyhow!("Browser requests unavailable"))?
            .insert(id, sender);
        let mut guard = RequestGuard {
            worker: self,
            id,
            completed: false,
        };
        let result = tokio::time::timeout(Duration::from_secs(20), async {
            self.tx
                .send(BrowserWorkerRequest { id, command })
                .await
                .map_err(|_| anyhow!("Browser input closed"))?;
            receiver.await.map_err(|_| anyhow!("Browser result lost"))?
        })
        .await
        .map_err(|_| anyhow!("Browser operation timed out; inspect before retrying"))?;
        guard.completed = true;
        result
    }
}
struct RequestGuard<'a> {
    worker: &'a Worker,
    id: i32,
    completed: bool,
}
impl Drop for RequestGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            if let Ok(mut pending) = self.worker.pending.lock() {
                pending.remove(&self.id);
            }
            self.worker.close();
        }
    }
}
