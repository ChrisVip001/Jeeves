//! One ordered request at a time. Shutdown interrupts a blocked read and reaps the child.
use anyhow::{Result, anyhow, ensure};
use jeeves_protocol::{DesktopWorkerCommand as Command, DesktopWorkerReply as Reply};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot, watch},
};

pub(super) struct Worker {
    tx: mpsc::Sender<(Command, oneshot::Sender<Result<Reply>>)>,
    stop: watch::Sender<bool>,
    exited: watch::Receiver<bool>,
}
impl Worker {
    pub fn start(path: &Path) -> Result<Self> {
        ensure!(path.is_absolute(), "Select an absolute desktop helper path");
        let mut child = tokio::process::Command::new(path)
            .env_clear()
            .envs(
                std::env::vars_os()
                    .filter(|(k, _)| matches!(k.to_str(), Some("HOME" | "TMPDIR" | "LANG"))),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let (tx, mut rx) = mpsc::channel::<(Command, oneshot::Sender<Result<Reply>>)>(1);
        let (stop, mut stopped) = watch::channel(false);
        let (done, exited) = watch::channel(false);
        tokio::spawn(async move {
            let serve = async {
                while let Some((command, reply)) = rx.recv().await {
                    let timeout = if matches!(command, Command::Select) {
                        90
                    } else {
                        15
                    };
                    let exchange = async {
                        let mut line = serde_json::to_vec(&command)?;
                        line.push(b'\n');
                        stdin.write_all(&line).await?;
                        line.clear();
                        (&mut stdout)
                            .take(1_500_001)
                            .read_until(b'\n', &mut line)
                            .await?;
                        ensure!(
                            line.len() <= 1_500_000 && line.last() == Some(&b'\n'),
                            "Incomplete desktop worker response"
                        );
                        Ok(serde_json::from_slice::<Reply>(&line)?)
                    };
                    let result = tokio::time::timeout(Duration::from_secs(timeout), exchange)
                        .await
                        .unwrap_or_else(|_| Err(anyhow!("Desktop worker timed out")));
                    let failed = result.is_err();
                    if reply.send(result).is_err() || failed {
                        break;
                    }
                }
            };
            tokio::select! {biased; _=stopped.changed()=>{}, _=serve=>{}}
            if child.kill().await.is_ok() || matches!(child.try_wait(), Ok(Some(_))) {
                done.send_replace(true);
            }
        });
        Ok(Self { tx, stop, exited })
    }
    pub(super) fn cancellation(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }
    pub fn is_closing(&self) -> bool {
        *self.stop.borrow()
    }
    pub fn close(&self) {
        self.stop.send_replace(true);
    }
    pub async fn wait(&self) -> Result<()> {
        self.exited
            .clone()
            .wait_for(|done| *done)
            .await
            .map_err(|_| anyhow!("Could not confirm desktop helper exit"))?;
        Ok(())
    }
    pub async fn request(&self, command: Command) -> Result<Reply> {
        ensure!(!*self.stop.borrow(), "Window selection revoked");
        let (tx, rx) = oneshot::channel();
        self.tx
            .send((command, tx))
            .await
            .map_err(|_| anyhow!("Desktop helper ended; reselect the window"))?;
        rx.await
            .map_err(|_| anyhow!("Desktop helper interrupted; check the action receipt"))?
    }
}
