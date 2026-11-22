//! Bind each model invocation to its runtime owner before contact with the provider.
use jeeves_provider::{CreateRequest, CreateResponse, Provider, ProviderError, Resolved, TextSink};
use std::{future::Future, pin::Pin, sync::Arc};

type Reply<'a> = Pin<Box<dyn Future<Output = Result<CreateResponse, ProviderError>> + Send + 'a>>;

pub(crate) fn metered(
    pool: &sqlx::SqlitePool,
    resolved: &Resolved,
    session: &str,
    turn: &str,
    run: Option<&str>,
    purpose: &'static str,
) -> Resolved {
    for_owner(pool, resolved, session, turn, None, run, purpose)
}

pub(crate) fn for_owner(
    pool: &sqlx::SqlitePool,
    resolved: &Resolved,
    session: &str,
    turn: &str,
    task: Option<&str>,
    run: Option<&str>,
    purpose: &'static str,
) -> Resolved {
    let meter = Meter {
        pool: pool.clone(),
        task: task.map(str::to_owned),
        inner: resolved.client.clone(),
        provider: resolved.provider_id.clone(),
        session: session.to_owned(),
        turn: turn.to_owned(),
        run: run.map(str::to_owned),
        purpose,
    };
    Resolved {
        client: Arc::new(meter),
        ..resolved.clone()
    }
}

struct Meter {
    task: Option<String>,
    pool: sqlx::SqlitePool,
    inner: Arc<dyn Provider>,
    provider: String,
    session: String,
    turn: String,
    run: Option<String>,
    purpose: &'static str,
}

/// A caller can drop an in-flight stream on interruption, timeout or grant revocation.
struct PendingRequest {
    id: i64,
    pool: sqlx::SqlitePool,
    finished: bool,
}
impl Drop for PendingRequest {
    fn drop(&mut self) {
        if !self.finished {
            let pool = self.pool.clone();
            let id = self.id;
            tokio::spawn(async move {
                if let Err(error) = sqlx::query("UPDATE model_requests SET termination='interrupted' WHERE id=? AND termination='in_flight'")
                    .bind(id).execute(&pool).await {
                    eprintln!("(jeeves) interrupted model request could not be closed: {error}");
                }
            });
        }
    }
}

fn db_error(error: sqlx::Error) -> ProviderError {
    ProviderError::Api(format!("Cannot record model request: {error}"))
}

impl Meter {
    async fn charge_reported(&self) -> Result<(), ProviderError> {
        if self.task.is_some() || self.run.is_some() {
            jeeves_state::Usage::new(self.pool.clone())
                .charge_pending(self.task.as_deref(), self.run.as_deref())
                .await
                .map_err(|e| {
                    ProviderError::Api(format!("Cannot charge reported model usage: {e}"))
                })?;
        }
        Ok(())
    }

    async fn invoke(
        &self,
        request: CreateRequest,
        stream: Option<TextSink>,
    ) -> Result<CreateResponse, ProviderError> {
        self.charge_reported().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO model_requests(session_id,turn_id,task_id,run_id,round_id,purpose,provider,model)
             VALUES(?,?,COALESCE(?,(SELECT task_id FROM task_runs WHERE id=?)),?,(SELECT round_id FROM turn_checkpoints WHERE turn_id=? AND run_id IS ? AND ?='execution'),?,?,?)
             RETURNING id"
        ).bind(&self.session).bind(&self.turn).bind(&self.task).bind(&self.run).bind(&self.run)
         .bind(&self.turn).bind(&self.run).bind(self.purpose)
         .bind(self.purpose).bind(&self.provider).bind(&request.model)
         .fetch_one(&self.pool).await.map_err(db_error)?;
        let mut pending = PendingRequest {
            id,
            pool: self.pool.clone(),
            finished: false,
        };
        let result = match stream {
            Some(sink) => self.inner.stream(request, sink).await,
            None => self.inner.create(request).await,
        };
        let usage = result.as_ref().ok().map(|r| r.usage);
        let termination = match &result {
            Ok(r) => serde_json::to_value(r.stop_reason).expect("stop reason is an enum"),
            Err(_) => serde_json::Value::String("failed".into()),
        };
        sqlx::query("UPDATE model_requests SET termination=?,input_tokens=?,output_tokens=?,cache_read=?,cache_creation=? WHERE id=? AND termination='in_flight'")
            .bind(termination.as_str()).bind(usage.map(|u|u.input_tokens)).bind(usage.map(|u|u.output_tokens))
            .bind(usage.map(|u|u.cache_read_input_tokens)).bind(usage.map(|u|u.cache_creation_input_tokens))
            .bind(id).execute(&self.pool).await.map_err(db_error)?;
        pending.finished = true;
        self.charge_reported().await?;
        result
    }
}
impl Provider for Meter {
    fn create(&self, request: CreateRequest) -> Reply<'_> {
        Box::pin(self.invoke(request, None))
    }
    fn stream(&self, request: CreateRequest, sink: TextSink) -> Reply<'_> {
        Box::pin(self.invoke(request, Some(sink)))
    }
}
