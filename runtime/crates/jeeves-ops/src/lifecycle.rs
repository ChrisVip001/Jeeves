//! Closing HTTP, the hosted gateway, and the engine concurrently.
use crate::{gateways::Gateways, AppServer};
use std::{future::Future, time::Duration};

/// Inside Kubernetes' default 30-second process-stop grace period.
const DRAIN_GRACE: Duration = Duration::from_secs(25);
const HTTP_DRAIN_GRACE: Duration = Duration::from_secs(3);

pub(crate) async fn drain<F>(app: &AppServer, gateways: &Gateways, http: Option<F>)
where
    F: Future<Output = std::io::Result<()>>,
{
    eprintln!(
        "(jeeves) stopping: finishing the turns in flight (up to {}s)",
        DRAIN_GRACE.as_secs()
    );
    let http_drain = async {
        if let Some(http) = http {
            match tokio::time::timeout(HTTP_DRAIN_GRACE, http).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("(jeeves) HTTP shutdown failed: {error}"),
                Err(_) => {
                    eprintln!("(jeeves) HTTP drain deadline reached; closing cockpit connections")
                }
            }
        }
    };
    let gateway_drain = async {
        if tokio::time::timeout(HTTP_DRAIN_GRACE, gateways.stop())
            .await
            .is_err()
        {
            eprintln!("(jeeves) gateway drain deadline reached");
        }
    };
    let (clean, (), ()) = tokio::join!(app.shutdown(DRAIN_GRACE), http_drain, gateway_drain);
    if !clean {
        eprintln!("(jeeves) the Runtime did not finish in time; forced stop requested; unfinished turns will be reconciled on the next start");
    }
}
