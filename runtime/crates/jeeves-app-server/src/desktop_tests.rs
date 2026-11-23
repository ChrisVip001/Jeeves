use crate::input_wake_tests::{fixture, rpc};
use serde_json::json;

#[tokio::test]
async fn desktop_executor_is_off_until_installed_and_selected() {
    let (_home, server, _) = fixture().await;
    let status = rpc(&server, "desktop/status", json!({"taskId":"input-task"}))
        .await
        .unwrap();
    assert_eq!(status["available"], false);
    assert_eq!(status["selected"], false);
    assert!(
        rpc(&server, "desktop/observe", json!({"taskId":"input-task"}))
            .await
            .is_err()
    );
}

#[cfg(unix)]
mod worker {
    use super::*;
    use crate::Server;
    use serde_json::Value;
    use std::{fs, os::unix::fs::PermissionsExt, time::Duration};
    async fn setup() -> (tempfile::TempDir, Server) {
        let (home, server, _) = fixture().await;
        let script = home.path().join("window");
        fs::write(&script, include_bytes!("../tests/fixtures/window.py")).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(home.path().join("mode"), "allow").unwrap();
        server.settings.write().unwrap().desktop_helper = Some(script);
        sqlx::query("UPDATE agent_tasks SET status='pending' WHERE id='input-task'")
            .execute(&server.pool)
            .await
            .unwrap();
        (home, server)
    }
    fn task() -> Value {
        json!({"taskId":"input-task"})
    }
    fn act(o: &Value) -> Value {
        json!({"taskId":"input-task","observationId":o["observationId"],"elementId":1,"action":"setText","text":"plain fixture"})
    }
    #[tokio::test]
    async fn desktop_native_dialog_has_no_selected_window_before_confirmation() {
        let (home, s) = setup().await;
        let mode = home.path().join("mode");
        fs::write(&mode, "choosing").unwrap();
        let operator = async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !home.path().join("dialog").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let status = rpc(&s, "desktop/status", task()).await.unwrap();
            assert_eq!(status["selected"], false);
            assert!(rpc(&s, "desktop/observe", task()).await.is_err());
            fs::write(mode, "allow").unwrap();
        };
        let (selected, ()) = tokio::join!(rpc(&s, "desktop/select", task()), operator);
        assert!(selected.is_ok());
        assert_eq!(
            rpc(&s, "desktop/status", task()).await.unwrap()["selected"],
            true
        );
    }
    #[tokio::test]
    async fn desktop_denial_stale_scope_receipts_and_takeover() {
        let (home, s) = setup().await;
        let mode = home.path().join("mode");
        fs::write(&mode, "deny").unwrap();
        assert!(
            rpc(&s, "desktop/select", task())
                .await
                .unwrap_err()
                .contains("permission")
        );
        fs::write(&mode, "allow").unwrap();
        let old = rpc(&s, "desktop/select", task()).await.unwrap();
        let o = rpc(&s, "desktop/observe", task()).await.unwrap();
        assert!(rpc(&s, "desktop/act", act(&old)).await.is_err());
        let mut bad = act(&o);
        bad["taskId"] = json!("other");
        assert!(rpc(&s, "desktop/act", bad).await.is_err());
        let mut bad = act(&o);
        bad["text"] = json!("text\n");
        assert!(rpc(&s, "desktop/act", bad).await.is_err());
        let mut bad = act(&o);
        bad["action"] = json!("click");
        assert!(rpc(&s, "desktop/act", bad).await.is_err());
        let next = rpc(&s, "desktop/act", act(&o)).await.unwrap();
        assert!(rpc(&s, "desktop/act", act(&o)).await.is_err());
        assert_eq!(
            fs::read_to_string(home.path().join("effects")).unwrap(),
            "setText\n"
        );
        fs::write(&mode, "deny").unwrap();
        assert!(rpc(&s, "desktop/act", act(&next)).await.is_err());
        let status = rpc(&s, "desktop/status", task()).await.unwrap();
        assert_eq!(status["receipts"][0]["status"], "rejected");
        assert_eq!(status["receipts"][1]["status"], "confirmed");
        fs::write(&mode, "allow").unwrap();
        let last = rpc(&s, "desktop/observe", task()).await.unwrap();
        fs::write(&mode, "switch").unwrap();
        assert!(rpc(&s, "desktop/act", act(&last)).await.is_err());
        let after_switch = rpc(&s, "desktop/status", task()).await.unwrap();
        assert_eq!(after_switch["receipts"][0]["status"], "rejected");
        assert!(rpc(&s, "desktop/observe", task()).await.is_err());
        assert_eq!(
            rpc(&s, "desktop/status", task()).await.unwrap()["selected"],
            false
        );
        fs::write(&mode, "allow").unwrap();
        rpc(&s, "desktop/select", task()).await.unwrap();
        s.browser.stop_task("other", false).await;
        assert_eq!(
            rpc(&s, "desktop/status", task()).await.unwrap()["selected"],
            true
        );
        rpc(&s, "desktop/takeover", task()).await.unwrap();
        assert!(rpc(&s, "desktop/observe", task()).await.is_err());
        rpc(&s, "desktop/select", task()).await.unwrap();
        rpc(&s, jeeves_protocol::methods::TASK_SUSPEND, task())
            .await
            .unwrap();
        assert!(rpc(&s, "desktop/select", task()).await.is_err());
        assert!(!s.browser.executor_status().await.unwrap().stopped);
        assert_eq!(
            rpc(&s, "desktop/status", task()).await.unwrap()["selected"],
            false
        );
    }
    #[tokio::test]
    async fn desktop_stop_interrupts_pending_effect_and_restart_preserves_unknown() {
        let (home, s) = setup().await;
        let o = rpc(&s, "desktop/select", task()).await.unwrap();
        fs::write(home.path().join("mode"), "block").unwrap();
        let stop = async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !home.path().join("effects").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let stopped = s.browser.emergency_stop().await.unwrap();
            assert!(rpc(&s, "desktop/select", task()).await.is_err());
            s.browser.release(stopped.generation).await.unwrap();
        };
        let (effect, ()) = tokio::join!(rpc(&s, "desktop/act", act(&o)), stop);
        assert!(effect.is_err());
        let recovered = Server::boot(s.pool.clone(), s.settings()).await;
        let status = rpc(&recovered, "desktop/status", task()).await.unwrap();
        assert_eq!(status["selected"], false);
        assert_eq!(status["receipts"][0]["status"], "unknown");
        assert!(rpc(&recovered, "desktop/act", act(&o)).await.is_err());
        assert_eq!(
            fs::read_to_string(home.path().join("effects")).unwrap(),
            "setText\n"
        );
    }
    #[tokio::test]
    async fn desktop_screen_share_requires_fresh_single_use_consent() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (_home, s) = setup().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dest = listener.local_addr().unwrap();
        let profile=serde_json::from_value(json!({"id":"screen-fixture","provider":"ollama","model":"local-fixture","vision":true,"base_url":format!("http://{dest}/v1")})).unwrap();
        s.models.write().unwrap().profiles.push(profile);
        let o = rpc(&s, "desktop/select", task()).await.unwrap();
        let capture = json!({"taskId":"input-task","observationId":o["observationId"]});
        assert!(rpc(&s, "desktop/frame", capture.clone()).await.is_err());
        let model = json!({"taskId":"input-task","profileId":"screen-fixture"});
        rpc(&s, "desktop/model", model.clone()).await.unwrap();
        let frame = rpc(&s, "desktop/frame", capture.clone()).await.unwrap();
        assert!(
            frame["destination"]
                .as_str()
                .unwrap()
                .contains(&dest.to_string())
        );
        let share = |f: &Value, confirmed| json!({"taskId":"input-task","frameId":f["frameId"],"confirmed":confirmed});
        assert!(
            rpc(&s, "desktop/share", share(&frame, false))
                .await
                .is_err()
        );
        rpc(&s, "desktop/model", model).await.unwrap();
        assert!(rpc(&s, "desktop/share", share(&frame, true)).await.is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(40), listener.accept())
                .await
                .is_err()
        );
        let frame = rpc(&s, "desktop/frame", capture).await.unwrap();
        let receive = async {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            loop {
                let mut b = [0; 4096];
                let n = stream.read(&mut b).await.unwrap();
                assert!(n > 0);
                data.extend_from_slice(&b[..n]);
                if let Some(i) = data.windows(4).position(|s| s == b"\r\n\r\n")
                    && let Ok(body) = serde_json::from_slice::<Value>(&data[i + 4..])
                {
                    assert!(body.to_string().contains("data:image/jpeg;base64,"));
                    assert!(body.get("tools").is_none());
                    break;
                }
            }
            let body = r#"{"choices":[{"message":{"role":"assistant","content":"Fixture window"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(rpc(&s, "desktop/share", share(&frame, true)), receive)
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap()["text"], "Fixture window");
        assert!(rpc(&s, "desktop/share", share(&frame, true)).await.is_err());
        let observation = rpc(&s, "desktop/observe", task()).await.unwrap();
        let frame = rpc(
            &s,
            "desktop/frame",
            json!({"taskId":"input-task","observationId":observation["observationId"]}),
        )
        .await
        .unwrap();
        let redirect = async {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            stream.write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{dest}/moved\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(rpc(&s, "desktop/share", share(&frame, true)), redirect)
        })
        .await
        .unwrap();
        assert!(result.is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(40), listener.accept())
                .await
                .is_err()
        );
    }
}
