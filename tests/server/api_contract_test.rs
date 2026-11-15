//! The v2 contract the CLI is compiled against is the one the pinned server
//! serves. This is the first test to fail when the Orion pin and the CLI's
//! decoded DTOs drift apart.

use orion_api::v2::{
    API_VERSION, DeploymentHead, EditionTier, ImplementationKind, NativeSchedulerStatus,
    ProviderBackendInfo, RuntimeCapabilitiesResponse,
};
use tower::ServiceExt as _;

use crate::common::{self, body_json, json_request};

/// `/runtime-capabilities` decodes into the pinned `RuntimeCapabilitiesResponse`
/// on a real node, and the vocabulary this CLI branches on round-trips.
#[tokio::test]
async fn the_pinned_v2_contract_is_what_the_node_serves() {
    let app = common::test_app().await;
    let resp = app
        .oneshot(json_request(
            "GET",
            "/api/v2/admin/runtime-capabilities",
            None,
        ))
        .await
        .expect("the router answers");
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;

    let capabilities: RuntimeCapabilitiesResponse =
        serde_json::from_value(body).expect("the pinned contract decodes the node's answer");

    assert_eq!(capabilities.api_version, API_VERSION);
    assert_eq!(capabilities.edition, EditionTier::Community);
    assert!(!capabilities.channel_drivers.is_empty());
    assert!(
        capabilities
            .channel_drivers
            .iter()
            .any(|protocol| protocol == "rest"),
        "a Community node serves REST: {capabilities:?}"
    );
    assert!(
        capabilities
            .provider_backends
            .iter()
            .any(|backend| backend.driver_id == "org.orion.sqlite"
                && backend.kind == ImplementationKind::Rust
                && backend.package_digest.is_some()),
        "the SQLite provider is registered with its digest: {capabilities:?}"
    );
    let _: DeploymentHead = capabilities.deployment_head;
    let _: NativeSchedulerStatus = capabilities.native_scheduler;
    assert!(!capabilities.node_status.node_id.is_empty());
    assert!(!capabilities.admission_policy_id.is_empty());

    // The edition vocabulary is ordered and its wire names are stable.
    for tier in EditionTier::ALL {
        assert_eq!(EditionTier::parse(tier.as_str()), Some(*tier));
    }
    assert!(EditionTier::Enterprise.admits(EditionTier::Community));
    assert!(EditionTier::Standard.admits(EditionTier::Standard));
    assert!(!EditionTier::Community.admits(EditionTier::Standard));

    // A provider backend's dependency declaration is part of the contract,
    // even when this node declares none.
    let backend = ProviderBackendInfo {
        kind: ImplementationKind::Rust,
        driver_id: "example".into(),
        package_digest: None,
        operations: None,
        dependencies: None,
    };
    assert_eq!(
        serde_json::to_value(&backend).expect("serializes"),
        serde_json::json!({"kind": "rust", "driver_id": "example"})
    );
}
