//! Controlled research replay: observable tool/dispatch behavior, not an LLM quality test.

use super::*;
use crate::input_wake_tests::rpc;
use jeeves_provider::mock::Mock;
use jeeves_tools::web::{fetch_url_with_primary, search_web_with_primary};
use serde_json::Value;

const ASK: &str = "查一下大桥坍塌后，巴尔的摩港在 2024 年 7 至 8 月是否恢复到 2023 年同期水平。分别核实航道、进港船只、港务局码头集装箱吞吐量。";
const QUERY: &str = "Baltimore port channel vessel arrivals MPA terminal TEU July August 2024";
const ACCESS: &str = "航道已于 6 月中旬恢复";
const TRAFFIC: &str = "7 至 8 月进港船只接近 2023 年同期";
const TEU: &str = "7 至 8 月港务局码头集装箱吞吐量仍远低于 2023 年同期";
const UNKNOWN: &str = "港务局码头集装箱吞吐量尚未核实";
const OVERCLAIM: &str = "港口已全面恢复";
const TASK: &str = "research-evidence";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Record {
    Traffic,
    Mirror,
    Access,
    Throughput,
}
use Record::*;

impl Record {
    fn url(self) -> String {
        let slug = match self {
            Traffic => "arrivals",
            Mirror => "mirror-vessel-arrivals",
            Access => "channel",
            Throughput => "terminal-teu",
        };
        // Public IP literal to pass the normal URL check without DNS; mock handler intercepts.
        format!("https://93.184.216.34/research-replay/{slug}")
    }

    fn text(self) -> &'static str {
        match self {
            Traffic | Mirror => TRAFFIC,
            Access => ACCESS,
            Throughput => TEU,
        }
    }

    fn family(self) -> &'static str {
        match self {
            Traffic | Mirror | Throughput => "mpa-october-minutes",
            Access => "usace-channel-restoration",
        }
    }

    fn fetch(self, id: &str) -> CreateResponse {
        Mock::tool_use(id, "fetch_url", json!({"url":self.url()}))
    }

    fn cite(self) -> String {
        format!("{} [来源]({})。", self.text(), self.url())
    }
}

fn install(
    server: &mut Server,
    ranking: &[Record],
    responses: Vec<CreateResponse>,
) -> (Arc<Mutex<Vec<String>>>, Arc<Mock>) {
    let log = Arc::new(Mutex::new(vec![]));
    let recorded = log.clone();
    let ranking = ranking.to_vec();
    let adapter: jeeves_tools::Handler = Arc::new(move |_, args| {
        let recorded = recorded.clone();
        let input = args["query"]
            .as_str()
            .or_else(|| args["url"].as_str())
            .unwrap()
            .to_string();
        let output = if input == QUERY {
            Ok(ranking
                .iter()
                .enumerate()
                .map(|(i, record)| {
                    format!(
                        "Source {}: {:?}\nURL: {}\nSnippet: Baltimore port update",
                        i + 1,
                        record,
                        record.url()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n"))
        } else {
            // A scripted attempt to fetch a withheld record is a broken fixture.
            let record = ranking
                .iter()
                .find(|record| record.url() == input)
                .unwrap_or_else(|| panic!("Unexpected fixture source: {input}"));
            Ok(record.text().to_owned())
        };
        Box::pin(async move {
            recorded.lock().unwrap().push(input);
            output
        })
    });
    let mut tools = server.tools.without("search_web").without("fetch_url");
    tools.register(search_web_with_primary(Some(adapter.clone())));
    tools.register(fetch_url_with_primary(Some(adapter)));
    server.tools = Arc::new(tools);
    let model = Arc::new(Mock::streaming(responses));
    server.install_provider(Resolved {
        client: model.clone(),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 32_000,
    });
    (log, model)
}

fn search() -> CreateResponse {
    Mock::tool_use("search", "search_web", json!({"query":QUERY}))
}

fn score(reply: &str, opened: &[String], throughput_available: bool) -> bool {
    let claims = reply.split('。').collect::<Vec<_>>();
    let supported = |record: Record| {
        let url = record.url();
        opened.contains(&url)
            && claims.iter().any(|clause| {
                clause.contains(record.text()) && clause.contains(&format!("]({url})"))
            })
    };
    !reply.contains(OVERCLAIM)
        && supported(Access)
        && (supported(Traffic) || supported(Mirror))
        && if throughput_available {
            supported(Throughput) && !reply.contains(UNKNOWN)
        } else {
            reply.contains(UNKNOWN) && !reply.contains(TEU)
        }
}

#[test]
fn a_same_family_copy_and_a_valid_but_narrow_citation_are_not_full_recovery() {
    assert_eq!(Traffic.family(), Mirror.family());
    assert_ne!(Traffic.family(), Access.family());
    let reply = Access.cite() + &Traffic.cite() + &Mirror.cite();
    let opened = [Access.url(), Traffic.url(), Mirror.url()];
    assert!(!score(&reply, &opened, true));
    assert!(!score(&(reply.clone() + &Throughput.cite()), &opened, true));
    assert!(score(&(reply.clone() + UNKNOWN), &opened, false));
    assert!(!score(&(reply + UNKNOWN + OVERCLAIM), &opened, false));
}

#[tokio::test]
async fn main_chat_replays_rank_swap_and_missing_throughput_under_same_cap() {
    for (arm, ranking) in [
        ("traffic-first", vec![Traffic, Mirror, Access, Throughput]),
        ("teu-first", vec![Throughput, Mirror, Access, Traffic]),
        ("no-teu", vec![Traffic, Mirror, Access]),
    ] {
        let (_home, mut server, _) = setup().await;
        let has_teu = ranking.contains(&Throughput);
        let requested = ranking
            .iter()
            .copied()
            .filter(|r| *r != Mirror)
            .collect::<Vec<_>>();
        let mut responses = vec![search()];
        for (i, record) in requested.iter().enumerate() {
            responses.push(record.fetch(&format!("read-{i}")));
        }
        let reply = Access.cite()
            + &Traffic.cite()
            + &if has_teu {
                Throughput.cite()
            } else {
                UNKNOWN.to_owned()
            };
        responses.push(Mock::text(&reply));
        let (log, model) = install(&mut server, &ranking, responses);
        let events = send(&server, ASK, arm, None).await.unwrap();
        let reply = events
            .iter()
            .find_map(|event| match event {
                ServerNotification::TurnCompleted(done) => Some(done.reply.as_str()),
                _ => None,
            })
            .unwrap();
        {
            let requests = model.received.lock().unwrap();
            let exposed = requests[1]
                .messages
                .iter()
                .flat_map(|m| &m.content)
                .find_map(|block| match block {
                    jeeves_provider::ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                    } if tool_use_id == "search" => Some(content),
                    _ => None,
                })
                .unwrap();
            let offsets = ranking
                .iter()
                .map(|r| exposed.find(&r.url()).unwrap())
                .collect::<Vec<_>>();
            assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
            assert_eq!(exposed.contains(&Throughput.url()), has_teu);
        }
        let log = log.lock().unwrap().clone();
        let expected = std::iter::once(QUERY.to_owned())
            .chain(requested.iter().map(|r| r.url()))
            .collect::<Vec<_>>();
        assert_eq!(log, expected);
        assert!(log.len() <= 4);
        assert!(score(reply, &log[1..], has_teu), "{arm}: {reply}");
        assert!(server.tasks.list(10).await.unwrap().is_empty());
    }
}

fn write(id: &str, path: &str, content: &str) -> CreateResponse {
    Mock::tool_use(id, "write_file", json!({"path":path,"content":content}))
}

async fn wait_for_status(server: &Server, wanted: jeeves_state::TaskStatus) {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let row = server.tasks.get(TASK).await.unwrap().unwrap();
            if row.status == wanted {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if result.is_err() {
        let row = server.tasks.get(TASK).await.unwrap().unwrap();
        panic!(
            "expected {wanted:?}, observed {:?}: {}",
            row.status, row.checks_json
        );
    }
}

async fn pause_after_first_read(
    ranking: &[Record],
) -> (tempfile::TempDir, std::path::PathBuf, String) {
    use jeeves_state::{NewTask, TaskStatus};
    let (home, mut server, _) = setup().await;
    let workspace = home.path().canonicalize().unwrap();
    let pending = r#"{"unresolved":["terminal throughput"]}"#;
    let (first_calls, _) = install(
        &mut server,
        ranking,
        vec![
            search(),
            Traffic.fetch("traffic"),
            write("checkpoint", "research-progress.json", pending),
            Mock::text("Draft only; terminal throughput remains unresolved."),
        ],
    );
    server.tasks.create(NewTask {
        id:TASK, session_id:Some("research-replay"), title:"Baltimore recovery",
        goal_condition:None, goal_draft:None,
        acceptance_json:r#"[{"kind":"fileExists","path":"report.md"},{"kind":"sourceSupports","path":"claims.json"}]"#,
        budget_json:"{}", cwd:Some(workspace.to_str().unwrap()), max_rounds:3, tier:None,
    }).await.unwrap();
    rpc(&server, "task/wait", json!({"taskId":TASK,"source":"replay-fixture","eventKey":"continue","description":"Pause after early source for controlled restart"})).await.unwrap();
    rpc(&server, "task/start", json!({"taskId":TASK,"message":ASK}))
        .await
        .unwrap();
    wait_for_status(&server, TaskStatus::Suspended).await;
    assert_eq!(
        *first_calls.lock().unwrap(),
        [QUERY.to_owned(), Traffic.url()]
    );
    assert!(!workspace.join("report.md").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.join("research-progress.json")).unwrap(),
        pending
    );
    let waits = jeeves_state::external_waits::ExternalWaits::new(server.pool.clone());
    let turn = waits.waiting(TASK).await.unwrap().unwrap().turn_id;
    (home, workspace, turn)
}

#[tokio::test]
async fn task_reopen_preserves_unresolved_throughput_and_the_original_fetch() {
    use jeeves_state::TaskStatus;
    let ranking = [Traffic, Mirror, Access, Throughput];
    // Dropping a Server handle alone does not stop its spawned input watcher.
    // Shut down its dedicated runtime before opening the same database again.
    let (_home, workspace, turn) = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(pause_after_first_read(&ranking))
    })
    .join()
    .unwrap();

    let pool = jeeves_state::open(&workspace.join("state.db"))
        .await
        .unwrap();
    let claims = [Access, Traffic, Throughput]
        .into_iter()
        .map(|r| json!({"claimId":r.url(),"text":r.text(),"sourceRefs":[r.url()],"quote":r.text()}))
        .collect::<Vec<_>>();
    let reply = Access.cite() + &Traffic.cite() + &Throughput.cite();
    let mut calls = None;
    let reopened = Server::boot_fixture(
        pool,
        jeeves_config::Settings {
            home: workspace.clone(),
            write_enabled: true,
            ..Default::default()
        },
        |server| {
            calls = Some(
                install(
                    server,
                    &ranking,
                    vec![
                        Throughput.fetch("teu"),
                        Access.fetch("channel"),
                        write("claims", "claims.json", &json!(claims).to_string()),
                        write("report", "report.md", &reply),
                        Mock::text("已核实三项指标。"),
                    ],
                )
                .0,
            );
        },
    )
    .await;
    let calls = calls.unwrap();
    let checkpoint = reopened.checkpoints.get(&turn).await.unwrap().unwrap();
    assert_eq!(checkpoint.status, "waiting_input");
    assert!(checkpoint.messages_json.contains(TRAFFIC));
    assert!(checkpoint.messages_json.contains("terminal throughput"));
    assert!(!workspace.join("report.md").exists());
    let signal = json!({"conditionId":format!("{TASK}:input"),"source":"replay-fixture","eventKey":"continue","eventId":"one","data":{"resume":true}});
    assert_eq!(
        rpc(&reopened, "task/signal", signal.clone()).await.unwrap()["duplicate"],
        false
    );
    wait_for_status(&reopened, TaskStatus::Done).await;
    assert_eq!(
        rpc(&reopened, "task/signal", signal).await.unwrap()["duplicate"],
        true
    );
    assert_eq!(*calls.lock().unwrap(), [Throughput.url(), Access.url()]);
    assert_eq!(reopened.runs.count_by_task(TASK).await.unwrap(), 1);
    let index = std::fs::read_to_string(workspace.join("sources/index.jsonl")).unwrap();
    assert_eq!(index.lines().count(), 3);
    let opened = index
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["url"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    let final_report = std::fs::read_to_string(workspace.join("report.md")).unwrap();
    assert!(score(&final_report, &opened, true));
}

mod live;
