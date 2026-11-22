//! One operator latch per Runtime. Releasing it never reopens a task or replays input.
use super::{BrowserHost, Session};
use anyhow::{anyhow, ensure, Result};
use jeeves_protocol::ExecutorControlView;
use std::sync::Arc;

impl BrowserHost {
    pub(crate) async fn executor_status(&self) -> Result<ExecutorControlView> {
        let pending = self.executor_gate.lock().await;
        ensure!(
            !*pending,
            "Emergency stop did not complete; retry stop before releasing"
        );
        self.read_control().await
    }

    async fn read_control(&self) -> Result<ExecutorControlView> {
        let (stopped, generation) =
            sqlx::query_as("SELECT stopped,generation FROM executor_control WHERE id=1")
                .fetch_one(&self.pool)
                .await?;
        Ok(ExecutorControlView {
            stopped,
            generation,
        })
    }

    pub(crate) async fn admitted_generation(&self, stop_pending: bool) -> Result<i32> {
        let state = self.read_control().await?;
        ensure!(
            !stop_pending && !state.stopped,
            "Executors are stopped; a human must release the emergency stop first"
        );
        Ok(state.generation)
    }

    pub(super) async fn register(&self, session: Arc<Session>, generation: i32) -> Result<()> {
        let pending = self.executor_gate.lock().await;
        ensure!(
            self.admitted_generation(*pending).await? == generation,
            "Executor control changed while opening the browser"
        );
        self.sessions
            .lock()
            .map_err(|_| anyhow!("Browser sessions unavailable"))?
            .insert(session.task.clone(), session);
        Ok(())
    }

    pub(crate) async fn emergency_stop(&self) -> Result<ExecutorControlView> {
        let mut pending = self.executor_gate.lock().await;
        *pending = true;
        let mut workers = Vec::new();
        for session in self
            .sessions
            .lock()
            .map_err(|_| anyhow!("Browser sessions unavailable"))?
            .values()
        {
            session.worker.close();
            workers.push(session.worker.clone());
        }
        let desktop = self.desktop.stop_matching(|_| true);
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query("UPDATE executor_control SET stopped=1,generation=generation+1 WHERE id=1 AND stopped=0")
            .execute(&mut *tx).await?.rows_affected();
        if changed == 1 {
            sqlx::query("INSERT OR IGNORE INTO browser_holds(task_id,url,title,origins_json)
                         SELECT task_id,url,title,origins_json FROM browser_sessions WHERE status NOT IN ('closed','needsAttention')")
                .execute(&mut *tx).await?;
            sqlx::query("UPDATE browser_sessions SET status='needsAttention', generation=generation+1, observed_at=NULL,
                          error='Emergency stop; release it and open a fresh browser before continuing', updated_at=datetime('now')
                         WHERE status NOT IN ('closed','needsAttention')")
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        self.desktop.finish_stop(desktop).await?;
        for worker in workers {
            worker.wait_closed().await?;
        }
        self.sessions
            .lock()
            .map_err(|_| anyhow!("Browser sessions unavailable"))?
            .clear();
        *pending = false;
        self.read_control().await
    }

    pub(crate) async fn release(&self, expected: i32) -> Result<ExecutorControlView> {
        let pending = self.executor_gate.lock().await;
        ensure!(
            !*pending,
            "Emergency stop did not complete; retry stop before releasing"
        );
        let changed = sqlx::query("UPDATE executor_control SET stopped=0,generation=generation+1 WHERE id=1 AND stopped=1 AND generation=?")
            .bind(expected).execute(&self.pool).await?.rows_affected();
        ensure!(
            changed == 1,
            "Stop generation changed; inspect the current state before releasing"
        );
        self.read_control().await
    }
}
