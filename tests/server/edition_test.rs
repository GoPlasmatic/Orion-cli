//! The edition contracts the CLI relies on: what a composed node reports, and
//! that the canary manifest the CLI builds is accepted on Standard and refused
//! on Community.

use axum::Router;
use axum::http::StatusCode;
use orion_api::v2::EditionTier;
use serde_json::{Value, json};
use tower::ServiceExt as _;

use crate::common::{self, body_json, json_request, try_upload};

async fn send(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(json_request(method, uri, Some(body)))
        .await
        .expect("the router answers");
    let status = resp.status();
    (status, body_json(resp).await)
}

fn marker_workflow(version: &str) -> Value {
    json!({
        "name": "canary",
        "tasks": [{
            "id": "mark",
            "name": "Mark",
            "function": { "name": "map", "input": { "mappings": [
                { "path": "data.version", "logic": version } ] } }
        }]
    })
}

fn channel_definition() -> Value {
    json!({
        "name": "canary-ch",
        "channel_type": "sync",
        "protocol": "rest",
        "methods": ["POST"],
        "route_pattern": "/canary"
    })
}

/// The two workflow revisions and a channel split 50/50 over them, in the
/// shape `ManifestMember.rollouts` gives the server.
async fn canary_manifest(app: &Router) -> Value {
    let upload = |kind: &'static str, definition: Value| {
        let app = app.clone();
        async move {
            let (status, body) =
                try_upload(&app, json!({"kind": kind, "definition": definition})).await;
            assert_eq!(status, StatusCode::CREATED, "{kind}: {body}");
            body["digest"].as_str().expect("a digest").to_string()
        }
    };
    let v1 = upload("workflow", marker_workflow("v1")).await;
    let v2 = upload("workflow", marker_workflow("v2")).await;
    let channel = upload("channel", channel_definition()).await;

    json!({
        "schema_version": 2,
        "providers": [],
        "members": [
            { "entity": {"kind": "workflow", "id": "canary", "revision": 1},
              "definition_digest": v1, "requires": [] },
            { "entity": {"kind": "workflow", "id": "canary", "revision": 2},
              "definition_digest": v2, "requires": [] },
            { "entity": {"kind": "channel", "id": "canary-ch", "revision": 1},
              "definition_digest": channel,
              "requires": [
                  {"kind": "workflow", "id": "canary", "revision": 1},
                  {"kind": "workflow", "id": "canary", "revision": 2}],
              "rollouts": [
                  {"workflow_revision": 1, "start_bucket": 0, "end_bucket": 50},
                  {"workflow_revision": 2, "start_bucket": 50, "end_bucket": 100}] }
        ]
    })
}

#[tokio::test]
async fn a_composed_node_reports_the_edition_it_serves() {
    for edition in [
        EditionTier::Community,
        EditionTier::Standard,
        EditionTier::Enterprise,
    ] {
        let app = common::test_app_with_edition(edition).await;
        let (status, body) =
            send(&app, "GET", "/api/v2/admin/runtime-capabilities", json!({})).await;
        assert_eq!(status, StatusCode::OK, "{edition}: {body}");
        assert_eq!(body["edition"], edition.to_string(), "{body}");
    }
}

#[tokio::test]
async fn a_standard_node_accepts_a_canary_and_routes_both_revisions() {
    let app = common::test_app_with_edition(EditionTier::Standard).await;
    let manifest = canary_manifest(&app).await;

    let (status, plan) = send(
        &app,
        "POST",
        "/api/v2/admin/deployment-plans",
        json!({"base_revision": 0, "manifest": manifest}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let digest = plan["reviewed_manifest_digest"]
        .as_str()
        .expect("a reviewed digest");

    let (status, applied) = send(
        &app,
        "POST",
        "/api/v2/admin/deployments",
        json!({
            "idempotency_key": "edition-canary",
            "base_revision": 0,
            "manifest": manifest,
            "reason": "edition canary test",
            "reviewed_manifest_digest": digest
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{applied}");

    // A 50/50 split over 60 requests: missing one side has probability 2^-59.
    let mut versions = std::collections::BTreeSet::new();
    for _ in 0..60 {
        let (status, body) =
            send(&app, "POST", "/api/v2/data/canary-ch", json!({"data": {}})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        versions.insert(
            body["data"]["version"]
                .as_str()
                .unwrap_or_else(|| panic!("every request matches a revision: {body}"))
                .to_string(),
        );
    }
    assert_eq!(
        versions.into_iter().collect::<Vec<_>>(),
        vec!["v1".to_string(), "v2".to_string()]
    );
}

#[tokio::test]
async fn a_community_node_refuses_the_same_canary_before_it_commits() {
    let app = common::test_app().await;
    let manifest = canary_manifest(&app).await;

    let (status, plan) = send(
        &app,
        "POST",
        "/api/v2/admin/deployment-plans",
        json!({"base_revision": 0, "manifest": manifest}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{plan}");
    assert_eq!(plan["error"]["code"], "TIER_NOT_ENABLED", "{plan}");

    let (status, body) = send(
        &app,
        "POST",
        "/api/v2/admin/deployments",
        json!({
            "idempotency_key": "edition-canary",
            "base_revision": 0,
            "manifest": manifest,
            "reason": "edition canary test",
            "reviewed_manifest_digest": format!("sha256:{}", "a".repeat(64))
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "TIER_NOT_ENABLED", "{body}");
}
