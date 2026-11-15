# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working in this repository.

## Project Overview

Orion CLI is a Rust CLI for the Orion services runtime ([GoPlasmatic/Orion](https://github.com/GoPlasmatic/Orion)). It is its own repository because the Community and Enterprise editions share it; it is built from the Orion Community crates, declared in `Cargo.toml` as git dependencies on one Orion release commit (`{ git = "https://github.com/GoPlasmatic/Orion", rev = "…" }`) — every orion-* line, the `orion-server` dev-dependency included, names the same rev, and `ORION_REV` in `.github/workflows/ci.yml` matches it. The v2.0 release has no tag yet, so the rev is the release commit Orion Ext is cut from; move the pin to the `v2.0.0` tag when it exists. To build against a local Orion checkout, a gitignored `.cargo/config.toml` `[patch."https://github.com/GoPlasmatic/Orion"]` maps each crate to a path (README, *Contributing*); never commit the `Cargo.lock` that patch produces. It carries the offline authoring tools (lint, compile, fmt, clippy, dry-run, test, sql check, plugin signing), deploys compiled definition sets as v2 deployments, sends data, and reads engine status, health, traces, schedules and the operational ledgers via HTTP against an Orion server. Definitions reach a node only through a deployment; the v1 entity commands (`workflows`, `channels`, `connectors`, `plugins`, `packages`) and `benchmark` are gone.

AI assistants drive this CLI through the **agent skill** at `skills/orion/` in the Orion repository, not through a tool server. `orion-cli mcp serve` and the whole `src/mcp/` tree were removed in 1.2.0: the HTTP transport exposed the full admin API on a port with no authentication of its own, and all 58 tools were hand-written mirrors of `commands::`. Do not reintroduce one — extend the skill instead, and keep anything discoverable at runtime (`functions list`, `--help`) out of it.

## Build & Development Commands

```bash
cargo build                                  # Build
cargo test                                   # All tests (needs ORION_EXAMPLES_DIR / ORION_BIN for the few that otherwise skip)
cargo fmt --check                            # Check formatting
cargo clippy --all-targets -- -D warnings    # Lint (CI treats warnings as errors)
```

### Tests

- `tests/authoring/` — the offline tools' suite, run through this crate's binary. Fixtures are in `tests/fixtures/` (copies of the Orion server crate's `plugins/`, `clippy/` and `clippy-fix/` fixtures these tests read, plus two example sets under `examples/`). The tests over the whole of Orion's `examples/packages` read `ORION_EXAMPLES_DIR` (an Orion checkout's `examples/`) and skip without it; CI checks Orion out at the tag. Its container-gated `sql_check_test` runs in the `containers` CI job.
- `tests/server/` — one binary: the CLI against an orion-server node assembled in-process through `orion::bootstrap::assemble` + `orion::server::build_router` (the `orion-server` dev-dependency; `common/` is the minimal slice of Orion's integration harness). `compiled_deployment_test` (what `compile` writes is what a server stores) and `plugin_signing_test` (`plugin sign` output accepted under `[plugins.trust]`).
- `tests/deployments_dir_e2e.rs` — the compile → apply flow as a `cargo test`, against the `orion-server` process `ORION_BIN` names; skips without it.

### E2E Tests

Shell-based (not `cargo test`), at `tests/e2e/`; needs jq and curl, an `orion-server` binary and Orion's `examples/`:

```bash
cargo install --git https://github.com/GoPlasmatic/Orion --rev <rev> orion-server --locked
ORION_EXAMPLES_DIR=/path/to/Orion/examples ./tests/e2e/run.sh

# Useful env vars:
# ORION_BIN=<path>     orion-server binary (default: orion-server on PATH)
# ORION_CLI=<path>     orion-cli binary (default: this repo's debug build)
# E2E_PORT=9090        Custom port
# E2E_DEBUG=1          Debug logging
# E2E_SKIP_BUILD=1     Skip cargo build, use the existing orion-cli
# E2E_KEEP_SERVER=1    Don't stop server after tests
```

11 test suites in `tests/e2e/suites/` and fixtures in `tests/e2e/fixtures/`. Every suite deploys what it needs the way an operator does: `new_set` / `set_workflow` / `set_channel` / `set_plugin` write a definition set, and `deploy_set` compiles it with `orion-cli compile` and sends it with `deployments apply -d` (helpers.sh). An apply replaces the head, so a suite needing several services deploys them as one set. Suite 06 is data-driven: scenario cases in Orion's `examples/use-cases/` (each deploys an example package) plus runtime-behaviour cases in `tests/e2e/cases/`. Suite 07 smoke-covers the read-only command groups, and suite 11 covers deployments themselves (plan, removal, rollback). Reading a trace needs the `trace_token` from the async submit.

## Architecture

**Rust 1.98+ (MSRV), edition 2024, async with Tokio.**

### Module Layout

- `src/main.rs` — Entry point, clap CLI definition with global flags (`--server`, `--api-key`, `--api-key-header`, `--change-context`, `--output`, `--quiet`, `--verbose`, `--no-color`, `--yes`, `--require-edition`). `build_client()` resolves the API key flag → `ORION_API_KEY` → `~/.orion/config.toml` via `OrionConfig::resolve_api_key`, and applies `--change-context` as the `X-Orion-Change-Context` header on every request. `server_client()` adds the optional edition preflight, so it runs once before the command and local authoring commands warn that they have no target.
- `src/client.rs` — thin presentation adapter over the shared `orion-client` crate's `OrionClient` (the transport itself — auth, 30s timeout, envelope parsing, typed `ClientError` — lives there); this wrapper translates typed errors into the CLI's terminal messages (hints, `[CODE] message`, field errors). Endpoint paths come from `orion_client::paths`, never format strings.
- `src/config.rs` — `OrionConfig` loaded from `~/.orion/config.toml` (server_url, default_output), plus `resolve_api_key()` for the flag → env → file chain
- `src/output.rs` — Output formatting: `print_table()` (tabled with rounded borders), `print_value()` (JSON/YAML)
- `src/utils.rs` — shared command helpers: `wait_for_trace()` (shared by `traces wait` and `send --wait`), `read_json_input()`, `confirm()`, the status and duration formatters, and `print_list_footer()`, the renderer every list goes through (says how many rows the page held, the `total` when `--include-total` asked for it, and `next page: --cursor …` when more follow)
- `src/commands/` — One file per command group, each defining clap subcommands and `execute()` async functions

### Command Modules

| Module | Key functionality |
|---|---|
| `data.rs` | Send data: sync (`--profile` renders `_orion.profile`), async (wait/timeout/trace tracking; handles null trace_id when tracing is off), `--raw` for `body_mode = "payload"` channels |
| `traces.rs` | Execution trace viewing and polling (shows `task_trace_json` when present); `--token` carries the per-submission capability token |
| `engine.rs` | Engine status: what this node serves, and its load issues |
| `functions.rs` | List registered workflow task functions and their input schemas |
| `health.rs` | Health check with component status, exit code 1 if degraded |
| `metrics.rs` | Raw Prometheus metrics retrieval |
| `audit_logs.rs` | List audit log entries, with the endpoint's full filter set (`--action`, `--resource-type`, `--resource-id`, `--principal`, `--start-time`, `--end-time`) |
| `backups.rs` | Create and list database backups (SQLite) |
| `cache.rs` | `invalidate <namespace>`: bump a channel response-cache namespace |
| `cron.rs` | The scheduled-run ledger: `status` (per-channel cursor, next fire, last result), `list` (filterable by `--channel-id`/`--status`), `get`, `retry` — another attempt at the same occurrence, keeping its id and `scheduled_for` — `cancel`, which settles a pending or running occurrence failed and frees its singleton slot, and `trigger <channel>`, a new occurrence *now* (`POST /api/v2/admin/channels/{id}/trigger`) |
| `dlq.rs` | Trace dead-letter queue: list, get, requeue, purge (v1.0) |
| `deployments.rs`, `providers.rs`, `connections.rs` (+ `v2.rs`, `deployment_dir.rs`) | The v2 surfaces over `orion_client::V2Admin`'s typed calls, one file per command group; `v2.rs` holds what they share (`load_request`, `require_api_version`, `capabilities`, `require_edition`) and the flow tests that drive all three against one fake server. Each first reads the server's `api_version` (`require_api_version`, called from `main.rs`) and refuses one this CLI does not speak; `capabilities` is the one typed read of `/runtime-capabilities`, and `require_edition` is the `--require-edition` preflight. `deployments capabilities` renders the answer as a human summary (edition, admission policy, drivers, provider backends, node status) while `-o json|yaml` stays the server's typed answer; `deployments` (capabilities/plan/apply/list/get/wait/rollback), `provider` (list/get/upload/admit/revoke/scaffold/lint/pack — `list` shows tier and admission/artifact state, `admit --lock` passes the provider lock), `connections` (list/get/create/probe — `probe` exits 2 when the backend is unhealthy; the table views show provider, bindings and locators, and `create --bind/--secret` merges repeatable `alias=connection`/`path=locator` pairs into the request). `deployments plan\|apply -d <dir>` deploys a directory `orion-cli compile` wrote: `deployment_dir.rs` reads the target's capabilities once, refuses a channel protocol the node does not serve or an API revision mismatch in the directory's `target.json` before uploading, uploads its definitions (each answered digest must be its file name), reuses or creates connection revisions, assigns member revisions against the target's history (kept when unchanged, taken back when an earlier revision had the same definition, else one past the highest), plans against the head and applies with the reviewed digest; an unchanged directory applies nothing. A hand-written `-f` manifest may split a channel's traffic with `rollouts`; `validate_rollouts` checks the partition and each revision's presence in `requires`, and `require_canary_edition` checks the target edition, before the request. A compiled directory cannot express a canary yet: it names one definition per workflow id. `compile --capabilities` writes `target.json` (CLI metadata, never sent), and without the flag removes a stale one |
| `authoring.rs`, `dry_run.rs`, `sql.rs`, `plugin.rs` | The offline authoring tools — `lint`, `compile`, `fmt`, `clippy`; `dry-run`, `test`; `sql check`; `plugin digest\|keygen\|pubkey\|sign\|verify` — over `orion-authoring`, `orion-engine` and `orion-plugin-host`. They read files and never build a client. A tool that consults the server's config takes `-c, --server-config`; path outputs are `-o, --out` (the global `--output` is the output format). `lint`/`compile`/`clippy`/`sql check` take `--capabilities FILE` (a `deployments capabilities -o json` answer): it supplies an edition's channel drivers and provider operations, and `check_provider_bindings` validates each connection's `bindings` against the provider's declared aliases. `compile` also records `target.json` for `apply -d` |
| `config.rs` | CLI config management (set-server, show, set key-value) |
| `completions.rs` | Shell completion generation (bash/zsh/fish/powershell/elvish) |

Shared logic lives in `src/utils.rs`. A command module holds its clap definitions, its own tables and detail views, and the verbs unique to it.

**Every list surface pages by cursor** (50 default, 1000 max, server-clamped): `--limit`, `--cursor` (the previous page's `next_cursor`) and `--include-total`. There is no `--offset` and no `--sort-by` — lists are newest-first and filters narrow them. A list command that omits the paging flags silently truncates at 50 with no way to see the rest.

### Key Patterns

- **Config precedence:** CLI flags > env vars (`ORION_SERVER_URL`, `ORION_API_KEY`, `ORION_API_KEY_HEADER`, `ORION_CHANGE_CONTEXT`, `NO_COLOR`) > `~/.orion/config.toml`. The env vars are declared on the clap args themselves, so `cli.server` already carries `ORION_SERVER_URL` — there is no second resolver for it.
- **Exit codes carry the answer.** `health` exits 1 when the server reports itself degraded, even though the HTTP status is **200**. A command must read the answer, not only the HTTP status, or a failing check looks like a passing one.
- **Output formats:** table (default), json, yaml — controlled by `--output` flag
- **Error handling:** `anyhow` throughout; `OrionClient` parses server error responses with codes/messages and renders the v0.2 structured `error.details[]` (field-pathed validation errors) and `request_id` when present
- **All commands are async** — Tokio runtime, reqwest for HTTP

### Dependencies

Core: `clap` (derive) for CLI, `tokio` for async, `serde`/`serde_json`/`serde_yaml`/`toml` for serialization, `anyhow` for errors, `colored` + `tabled` for terminal output, and two of the Orion library crates: `orion-api` for the shared wire contract — the error envelope (`ErrorEnvelope`, `codes`) and the status vocabulary (`STATUS_ACTIVE`…) are the same definitions the server serializes — and `orion-client` for the HTTP transport (`OrionClient`, `paths`, `query_string`; it owns reqwest — the CLI has no direct HTTP dependency).

### CI/CD

Workflows in `.github/workflows/`:

- **CI** (`ci.yml`): fmt, clippy `-D warnings`, test (toolchain 1.100), MSRV `cargo check` on 1.98 (full test weekly), the container-gated `sql check` tests, an `orion-server` installed from the Orion rev (`ORION_REV`, cached per rev) feeding the `e2e` job (`tests/e2e` + `deployments_dir_e2e`) and the `examples` job (Orion's `examples/` linted, tested offline and deployed with this CLI), and a no-push Docker build
- **Release** (`release.yml` + `dist-workspace.toml`): a `vX.Y.Z` tag drives cargo-dist v0.31.0 cross-platform builds (macOS ARM, Linux x86_64/ARM, Windows) with shell/powershell/homebrew installers, gated on a green CI run (`ci-gate.yml`); the build toolchain is pinned to 1.100
- **Docker** (`docker-release-cli.yml`): the same tag builds the ghcr.io/goplasmatic/orion-cli image from the standalone `Dockerfile`
- **Homebrew tap:** GoPlasmatic/homebrew-tap
