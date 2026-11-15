//! The contract between `orion-cli plugin sign` and a node's
//! `[plugins.trust]` upload check, with the fixture component in
//! `tests/fixtures/plugins/`.

use axum::http::StatusCode;
use base64::Engine as _;
use serde_json::{Value, json};

use crate::common::{self, test_state_with_config};
use orion::config::AppConfig;
use orion::runtime::state::AppState;

const COMPONENT: &[u8] = include_bytes!("../fixtures/plugins/fixture.wasm");
// The upload manifest: the functions that answer a probe. `fixture.toml`
// also declares the three that misbehave on purpose, which the self-test
// every upload runs would — correctly — refuse.
const MANIFEST: &str = include_str!("../fixtures/plugins/fixture-upload.toml");

fn config() -> AppConfig {
    let mut config = AppConfig::default();
    config.trace_storage.mode = orion::config::TraceStorageMode::Sync;
    config.plugins.enabled = true;
    config.plugins.max_timeout_ms = 2_000;
    config
}

/// The fixture as a plugin definition, with `manifest` as its `plugin.toml`
/// text and an optional detached signature.
fn plugin_definition(manifest: &str, signature: Option<&str>) -> Value {
    let mut definition = json!({
        "manifest": manifest,
        "component": base64::engine::general_purpose::STANDARD.encode(COMPONENT),
        "tags": ["fixture"],
    });
    if let Some(signature) = signature {
        definition["signature"] = json!(signature);
    }
    json!({"kind": "plugin", "definition": definition})
}

/// An app whose node names trust keys: an upload must carry a signature over
/// the digest by one of them.
async fn trusting_app(keys: Vec<String>) -> (AppState, axum::Router) {
    let mut config = config();
    config.plugins.trust.public_keys = keys;
    let state = test_state_with_config(config).await;
    let router = orion::server::build_router(state.clone());
    (state, router)
}

/// A `.sig` file the verb wrote is accepted, as-is, by a node that trusts the
/// key `plugin keygen` printed.
#[tokio::test]
async fn a_signature_from_plugin_sign_is_accepted_at_upload() {
    let bin = common::orion_cli_bin();
    let dir = std::env::temp_dir().join(format!("orion-plugin-sign-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let key = dir.join("signer.pem");
    let component = dir.join("fixture.wasm");
    std::fs::write(&component, COMPONENT).expect("component");
    let run = |args: &[&std::ffi::OsStr]| {
        let out = std::process::Command::new(&bin)
            .args(args)
            .env_remove("ORION_SIGNING_KEY")
            .output()
            .expect("invoke orion-cli");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    run(&[
        "plugin".as_ref(),
        "keygen".as_ref(),
        "-o".as_ref(),
        key.as_os_str(),
    ]);
    let public = run(&[
        "plugin".as_ref(),
        "pubkey".as_ref(),
        "--key".as_ref(),
        key.as_os_str(),
    ]);
    run(&[
        "plugin".as_ref(),
        "sign".as_ref(),
        component.as_os_str(),
        "--key".as_ref(),
        key.as_os_str(),
    ]);
    let signature = std::fs::read_to_string(dir.join("fixture.wasm.sig")).expect("sig");
    let _ = std::fs::remove_dir_all(&dir);

    let (_state, app) = trusting_app(vec![public.trim().to_string()]).await;
    let (status, body) =
        common::try_upload(&app, plugin_definition(MANIFEST, Some(&signature))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}
