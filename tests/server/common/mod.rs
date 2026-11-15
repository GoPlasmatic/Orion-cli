//! The in-process node these suites drive — the part of orion-server's own
//! integration harness (`tests/integration/common`) they need.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use orion::config::AppConfig;
use orion::runtime::state::AppState;

/// The compiled `orion-cli` binary. `env!` rather than a runtime lookup:
/// cargo guarantees `CARGO_BIN_EXE_<name>` at compile time for an
/// integration test of the crate that owns the binary.
pub fn orion_cli_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_orion-cli"))
}

/// A node on an in-memory SQLite database, trace persistence pinned to
/// `sync` so a test never races a background flush.
pub async fn test_app() -> Router {
    let mut config = AppConfig::default();
    config.trace_storage.mode = orion::config::TraceStorageMode::Sync;
    orion::server::build_router(test_state_with_config(config).await)
}

/// A node whose composition reports `edition`, for the contracts a Standard
/// and Enterprise node serves and Community refuses.
pub async fn test_app_with_edition(edition: orion_api::v2::EditionTier) -> Router {
    let mut config = AppConfig::default();
    config.trace_storage.mode = orion::config::TraceStorageMode::Sync;
    orion::server::build_router(test_state_with_edition(edition).await)
}

pub async fn test_state_with_edition(edition: orion_api::v2::EditionTier) -> AppState {
    let mut config = AppConfig::default();
    config.trace_storage.mode = orion::config::TraceStorageMode::Sync;
    test_state_with_composition(
        config,
        orion::extensions::CompositionRoot::community().with_edition(edition),
    )
    .await
}

/// A full `AppState` through the server's real boot path,
/// `orion::bootstrap::assemble`, so wiring cannot drift from production.
///
/// Deliberate divergences from production bootstrap, as in the server's own
/// harness: storage falls back to in-memory SQLite (migrated by
/// `init_pool`), the process-global metrics recorder is first-caller-wins,
/// and the DLQ retry loop is off.
pub async fn test_state_with_config(config: AppConfig) -> AppState {
    test_state_with_composition(config, orion::extensions::CompositionRoot::community()).await
}

async fn test_state_with_composition(
    mut config: AppConfig,
    composition: orion::extensions::CompositionBuilder,
) -> AppState {
    let storage_config = if config.storage.url.is_empty()
        || config.storage.url == orion::config::StorageConfig::default().url
    {
        orion::config::StorageConfig {
            url: "sqlite::memory:".to_string(),
            max_connections: 5,
            ..config.storage.clone()
        }
    } else {
        orion::config::StorageConfig {
            max_connections: Ord::max(config.storage.max_connections, 5),
            ..config.storage.clone()
        }
    };
    let pool = orion::storage::init_pool(&storage_config).await.unwrap();
    config.trace_queue.dlq_retry_enabled = false;
    let metrics_handle = orion::metrics::init_metrics_with_instance(None);
    orion::bootstrap::assemble(
        orion::bootstrap::AssembleParams {
            config: Arc::new(config),
            pool,
            metrics_handle,
        },
        composition,
    )
    .await
    .expect("assemble app state")
    .state
}

pub fn json_request(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let body = match body {
        Some(v) => Body::from(serde_json::to_string(&v).unwrap()),
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

pub async fn body_json(response: axum::http::Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Upload `definition` (`{"kind": …, "definition": {…}}`), answering the
/// status and body as they are.
pub async fn try_upload(app: &Router, definition: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v2/admin/definitions",
            Some(definition),
        ))
        .await
        .expect("the router answers");
    let status = resp.status();
    (status, body_json(resp).await)
}

/// Self-cleaning scratch directory, unique per call even when tests run in
/// parallel within one microsecond.
pub struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    pub fn new(label: &str) -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "orion_cli_server_test_{label}_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self(path)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
