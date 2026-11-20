use super::*;

fn grant() -> Grant {
    Grant {
        id: "g1".into(),
        account: "owner".into(),
        resource: "feishu".into(),
        action: "send".into(),
        destination: "oc_1".into(),
        content_hash: None,
        scope: "always".into(),
        expires_at: None,
        revoked_version: 0,
        created_at: 0,
    }
}

fn req<'a>() -> GrantRequest<'a> {
    GrantRequest {
        account: "owner",
        resource: "feishu",
        action: "send",
        destination: "oc_1",
        content_hash: None,
        now: 100,
    }
}

#[test]
fn grant_covers_table() {
    let mut hashed = grant();
    hashed.content_hash = Some("h1".into());
    let mut expired = grant();
    expired.expires_at = Some(100);
    let mut live = grant();
    live.expires_at = Some(101);
    let mut revoked = grant();
    revoked.revoked_version = 1;
    let cases: Vec<(&str, Grant, GrantRequest, bool)> = vec![
        ("exact", grant(), req(), true),
        (
            "other account",
            grant(),
            GrantRequest {
                account: "x",
                ..req()
            },
            false,
        ),
        (
            "other resource",
            grant(),
            GrantRequest {
                resource: "mail",
                ..req()
            },
            false,
        ),
        (
            "other action",
            grant(),
            GrantRequest {
                action: "read",
                ..req()
            },
            false,
        ),
        (
            "other destination",
            grant(),
            GrantRequest {
                destination: "oc_2",
                ..req()
            },
            false,
        ),
        (
            "hash match",
            hashed.clone(),
            GrantRequest {
                content_hash: Some("h1"),
                ..req()
            },
            true,
        ),
        (
            "hash changed",
            hashed.clone(),
            GrantRequest {
                content_hash: Some("h2"),
                ..req()
            },
            false,
        ),
        ("hash missing", hashed, req(), false),
        (
            "unhashed grant ignores content",
            grant(),
            GrantRequest {
                content_hash: Some("z"),
                ..req()
            },
            true,
        ),
        ("expired", expired, req(), false),
        ("not yet expired", live, req(), true),
        ("revoked", revoked, req(), false),
    ];
    for (name, g, r, want) in cases {
        assert_eq!(grant_covers(&g, &r), want, "{name}");
    }
}

async fn store() -> (Grants, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::open(&dir.path().join("state.db")).await.unwrap();
    (Grants::new(pool), dir)
}

#[tokio::test]
async fn once_grant_is_spent_and_revoked_never_covers() {
    let (grants, _d) = store().await;
    let mut once = grant();
    once.scope = "once".into();
    grants.add(&once).await.unwrap();
    assert!(grants.consume(&req()).await.unwrap().is_some());
    assert!(grants.consume(&req()).await.unwrap().is_none(), "spent");
}

#[tokio::test]
async fn concurrent_consumers_can_spend_once_grant_only_once() {
    let (grants, _d) = store().await;
    let mut once = grant();
    once.scope = "once".into();
    grants.add(&once).await.unwrap();
    let request = req();
    let (a, b) = tokio::join!(grants.consume(&request), grants.consume(&request));
    assert_ne!(a.unwrap().is_some(), b.unwrap().is_some());
}

#[tokio::test]
async fn pending_restore_bumps_version_and_drops_expired() {
    let (grants, _d) = store().await;
    let row = |id: &str, exp| PendingRow {
        turn_id: "t".into(),
        request_id: id.into(),
        session_id: "s".into(),
        tool: "write_file".into(),
        args_preview: "a".into(),
        reason: "r".into(),
        version: 1,
        expires_at: exp,
    };
    grants.pending_put(&row("live", 500)).await.unwrap();
    grants.pending_put(&row("old", 50)).await.unwrap();
    let back = grants.pending_restore(100).await.unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].request_id, "live");
    assert_eq!(back[0].version, 2);
}
