//! A fallback chain: when the main model's provider is down or refuses the account, ask the
//! next model the person listed instead of ending the turn with an error.
//!
//! The chain sits **above** the per-provider retries (`retry.rs`): each provider first spends
//! its own bounded retries, and only the error that survives them moves the call along.
//!
//! What moves a call to the next target, and what does not:
//! * moves: errors a different provider can fix, namely rate limits and server faults that
//!   survived retries, network failures, a stream that broke before any text, and HTTP
//!   401 / 402 / 403 (bad key, unpaid quota, account blocked);
//! * stays: context overflow (the loop compacts instead), parse errors, capability refusals and
//!   any other 4xx. A backup would fail the same way or hide a request bug;
//! * stays: a stream that already showed text, because a second answer would print after the first;
//! * skipped: a backup that refuses the request (a smaller window, another vendor's
//!   continuation data). The chain moves on and reports the error that started the switch.
//!
//! Every switch is announced through the turn's retry outlet (`turn/retry`). Once a target
//! answers, later calls through the same chain start there, so a dead primary does not cost
//! every round its full retry budget. App-server builds one chain per turn, so the next turn
//! tries the primary again.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{
    ContentBlock, CreateRequest, CreateResponse, Provider, ProviderError, TextSink, retry,
};

type Reply<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<CreateResponse, ProviderError>> + Send + 'a>,
>;

/// One model the chain can ask: the primary first, then the person's backups in order.
#[derive(Clone)]
pub struct Target {
    /// What the switch notice calls it (the profile id).
    pub label: String,
    pub client: Arc<dyn Provider>,
    pub model: String,
    pub small_model: String,
    pub vision: bool,
    /// This model's output ceiling. The turn sized `max_tokens` for the primary, and a backup
    /// with a smaller limit would refuse the request outright.
    pub max_output_tokens: Option<i32>,
}

pub struct Fallback {
    chain: Vec<Target>,
    /// Index of the target that answered last; later calls start there.
    current: AtomicUsize,
}

/// How one attempt ended, when it did not answer.
enum Failure {
    /// Another provider might succeed.
    Switchable(ProviderError),
    /// This provider will not take the request (a request error, nothing shown yet).
    Refused(ProviderError),
    /// Text already reached the person: stop here, whatever comes next would repeat it.
    Shown(ProviderError),
}

impl Fallback {
    /// A chain of `primary` followed by `backups`. With no backups it behaves exactly like the primary.
    pub fn new(primary: Target, backups: Vec<Target>) -> Self {
        Self {
            chain: std::iter::once(primary).chain(backups).collect(),
            current: AtomicUsize::new(0),
        }
    }

    /// Errors a different provider could get past. See the module notes for the reasoning.
    pub fn switchable(error: &ProviderError) -> bool {
        error.retryable()
            || matches!(
                error,
                ProviderError::Http {
                    status: 401..=403,
                    ..
                }
            )
    }

    /// The model name to send to `target`: the primary's main and small models map onto the
    /// target's; any other name is only meaningful to the primary.
    fn model_for(&self, requested: &str, target: &Target) -> Option<String> {
        let primary = &self.chain[0];
        if requested == primary.model {
            Some(target.model.clone())
        } else if requested == primary.small_model {
            Some(target.small_model.clone())
        } else {
            None
        }
    }

    async fn attempt(
        &self,
        index: usize,
        request: &CreateRequest,
        sink: Option<&TextSink>,
    ) -> Result<CreateResponse, Failure> {
        let target = &self.chain[index];
        let mut request = request.clone();
        if index > 0
            && let Some(model) = self.model_for(&request.model, target)
        {
            request.model = model;
            if let Some(cap) = target.max_output_tokens {
                request.max_tokens = request.max_tokens.min(cap.max(1));
            }
        }
        let shown = Arc::new(AtomicBool::new(false));
        let result = match sink {
            Some(sink) => {
                let (shown, sink) = (shown.clone(), sink.clone());
                let watched: TextSink = Arc::new(move |piece: &str| {
                    shown.store(true, Ordering::Release);
                    sink(piece);
                });
                target.client.stream(request, watched).await
            }
            None => target.client.create(request).await,
        };
        result.map_err(|error| {
            if shown.load(Ordering::Acquire) {
                Failure::Shown(error)
            } else if Self::switchable(&error) {
                Failure::Switchable(error)
            } else {
                Failure::Refused(error)
            }
        })
    }

    async fn call(
        &self,
        request: CreateRequest,
        sink: Option<TextSink>,
    ) -> Result<CreateResponse, ProviderError> {
        if self.model_for(&request.model, &self.chain[0]).is_none() {
            return self.direct(request, sink).await;
        }
        let has_image = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|block| matches!(block, ContentBlock::Image { .. }));
        let start = match self.current.load(Ordering::Acquire) {
            index if has_image && !self.chain[index].vision => 0,
            index => index,
        };
        let mut error = match self.attempt(start, &request, sink.as_ref()).await {
            Ok(response) => return Ok(response),
            Err(Failure::Refused(error) | Failure::Shown(error)) => return Err(error),
            Err(Failure::Switchable(error)) => error,
        };
        for (index, target) in self.chain.iter().enumerate().skip(start + 1) {
            if has_image && !target.vision {
                continue;
            }
            retry::note_fallback(&error, index as u32, &target.label);
            match self.attempt(index, &request, sink.as_ref()).await {
                Ok(response) => {
                    self.current.store(index, Ordering::Release);
                    return Ok(response);
                }
                Err(Failure::Shown(last)) => return Err(last),
                Err(Failure::Switchable(last)) => error = last,
                Err(Failure::Refused(why)) => {
                    eprintln!(
                        "(jeeves) fallback {} refused the request: {why}",
                        target.label
                    );
                }
            }
        }
        Err(error)
    }

    /// A call the chain does not apply to (a model only the primary knows) goes straight there.
    async fn direct(
        &self,
        request: CreateRequest,
        sink: Option<TextSink>,
    ) -> Result<CreateResponse, ProviderError> {
        let client = &self.chain[0].client;
        match sink {
            Some(sink) => client.stream(request, sink).await,
            None => client.create(request).await,
        }
    }
}

impl Provider for Fallback {
    fn create(&self, request: CreateRequest) -> Reply<'_> {
        Box::pin(self.call(request, None))
    }

    fn stream(&self, request: CreateRequest, on_text: TextSink) -> Reply<'_> {
        Box::pin(self.call(request, Some(on_text)))
    }
}

#[cfg(test)]
#[path = "fallback_tests.rs"]
mod fallback_tests;
