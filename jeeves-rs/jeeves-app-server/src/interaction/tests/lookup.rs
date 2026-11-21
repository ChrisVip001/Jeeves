use super::*;

pub(super) const QUERY: &str = "我之前交办的任务有哪些？";

#[tokio::test]
async fn main_task_list_finds_older_owned_work_among_newer_unrelated_tasks() {
    let (_home, server, _) = setup().await;
    send(&server, "归纳会议议题", "create-agenda", None)
        .await
        .unwrap();
    let original = server.tasks.list(1).await.unwrap().pop().unwrap();
    wait(&server, &original.id).await;
    let dispatch = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone());
    let original_session = dispatch
        .get(&original.id)
        .await
        .unwrap()
        .unwrap()
        .session_id;
    send(&server, "整理下周例会议题", "create-next-agenda", None)
        .await
        .unwrap();
    let newer = server.tasks.list(1).await.unwrap().pop().unwrap();
    wait(&server, &newer.id).await;
    for index in 0..100 {
        server
            .tasks
            .create(jeeves_state::NewTask {
                id: &format!("other-{index}"),
                session_id: Some("another-conversation"),
                title: "Unrelated fixture",
                goal_condition: None,
                acceptance_json: "[]",
                budget_json: "{}",
                cwd: None,
                max_rounds: 1,
                tier: None,
            })
            .await
            .unwrap();
    }
    let events = send(&server, QUERY, "find-agenda", None).await.unwrap();
    let listed = events
        .iter()
        .find_map(|event| match event {
            ServerNotification::ToolCompleted(call) if call.tool_use_id == "list" => {
                Some(&call.output)
            }
            _ => None,
        })
        .unwrap();
    let entries: Vec<_> = listed.lines().collect();
    assert_eq!(entries.len(), 2, "{listed}");
    for (entry, task) in entries.iter().zip([&newer, &original]) {
        assert!(
            entry.contains(&task.id) && entry.contains(&task.title),
            "{listed}"
        );
    }

    send(
        &server,
        "补充参会名单",
        "follow-agenda",
        Some(original.id.clone()),
    )
    .await
    .unwrap();
    wait(&server, &original.id).await;
    assert_eq!(
        server.tasks.get(&original.id).await.unwrap().unwrap().cwd,
        original.cwd
    );
    assert_eq!(
        dispatch
            .get(&original.id)
            .await
            .unwrap()
            .unwrap()
            .session_id,
        original_session
    );
    assert_eq!(server.runs.count_by_task(&original.id).await.unwrap(), 2);
    assert_eq!(server.runs.count_by_task(&newer.id).await.unwrap(), 1);
}
