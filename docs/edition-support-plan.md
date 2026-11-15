# Orion CLI v2 Edition Support — Implementation Plan

**Status:** Draft for review  
**Date:** 2026-11-15  
**Scope:** `Orion-cli` support for the Community, Standard and Enterprise editions of Orion v2.  
**Related repositories:** `../Orion` (Community, v2.0), `../Orion-ext` (Standard + Enterprise, v2.0), `../Orion/skills/orion` (agent skill).

## Progress

- [x] Phase 0 — rebase on the Orion v2.0 release commit
- [x] Phase 1 — edition discovery and preflight
- [x] Phase 2 — authoring against an edition
- [x] Phase 3 — deployment manifest fidelity (bindings and `-f` canary; `-d` canary blocked on the upstream compiled format)
- [x] Phase 4 — provider and connection UX
- [x] Phase 5 — edition channel protocols (GraphQL via `--raw` and ingress guidance; SSE consume and custom transport headers deferred to `orion-client`)
- [x] Phase 6 — cluster operations
- [ ] Phase 7 — CI, docs, skill and release

---

## 1. Goal

One `orion-cli` binary must work against every Orion v2 edition:

- **Community** — the open-source `orion-server` composition.
- **Standard** and **Enterprise** — the `orion-ext` artifact. Both are the same software and differ only in support terms; `[extensions.licence]` decides the label the node reports.

The CLI must not grow an edition-conditional build. `Orion-ext` explicitly forbids edition-specific code paths, and the CLI is built from Community crates only. Edition support is therefore **runtime discovery plus feature gating**:

1. Read `GET /api/v2/admin/runtime-capabilities` (`edition`, `channel_drivers`, `channel_driver_info`, `provider_backends`, `admission_policy_id`, `node_status`).
2. Validate offline work against those capabilities (`--capabilities FILE`).
3. Expose edition-scoped features (canary rollout, cluster mode, edition drivers/providers) with a clean preflight, while leaving the server authoritative.
4. Give actionable errors when the target edition does not serve a feature.

### Assumptions

- "Enterprise support" means **Standard + Enterprise**, because `Orion-ext` ships them as one artifact and one feature set. The CLI treats both as `edition >= standard`; it must not assume Enterprise-only behaviour beyond the reported label.
- The CLI remains a single public artifact. There is no private CLI build, and no `--features enterprise`.
- The target pin is the Orion `v2.0.0` tag once published. Until then the pin is the release SHA both repositories are cut from.

---

## 2. What each edition serves

| Capability | Community | Standard / Enterprise | CLI today |
|---|---|---|---|
| v2 deployments, workflows, channels, plugins, connections | Yes | Yes | Yes |
| Channel protocols | `rest`, `http`, `cron` | plus `kafka` (ingress), `sse` and `graphql` (HTTP hooks) | `compile/lint --capabilities`; `send` is REST-shaped |
| Connector providers | HTTP, SQL (SQLite/Postgres/MySQL), memory cache | plus SMTP, Kafka, Redis, S3, MongoDB, Elasticsearch, ONNX models | Connections are JSON; provider `bindings` are dropped on apply |
| Canary rollout | Refused (`TIER_NOT_ENABLED`) | Allowed, multiple workflow revisions per channel | Hand-written `-f` manifests only; compiled directories cannot express it |
| Cluster mode | Not bootable (no cluster store) | Redis-backed `SharedStateStore` | `deployments wait --target observed`, otherwise little node visibility |
| Provider package tiers | `community` admitted | `standard` and below admitted (`orion.ext.admission/0`) | `provider list` hides tier/revocation; no lock on admit |
| Package verifier | Community verifier | `ExtPackageVerifier` | `admission_policy_id` printed only in capabilities JSON |

---

## 3. Current state

Already working, and worth keeping:

- `orion_client::V2Admin` covers every v2 admin call; the CLI never builds v2 paths or bodies.
- `deployments capabilities` prints the whole `RuntimeCapabilitiesResponse` as JSON, so a capabilities file can already be produced with `-o json`.
- `lint`, `compile`, `clippy` and `sql check` accept that file via `--capabilities` and build a Community-plus-edition `Boundary` for channel drivers and provider operations.
- `send` reaches REST/HTTP channels; `--raw` already sends a body verbatim, which is enough for a GraphQL POST.
- `deployments wait --target observed` exists; `/health` prints `components.cluster_store` when present.
- The server test harness assembles the real boot path and can compose an edition via `CompositionRoot::community().with_edition(...)`.

The structural gaps are in preflight, manifest fidelity and edition-facing UX — not in the transport.

---

## 4. Gap analysis

| # | Gap | Severity | Evidence |
|---|---|---|---|
| 1 | Pinned to `v2.0.0-rc.1`, a tag that does not exist on the remote and is not the v2.0 release commit | Blocker | `Cargo.toml:50-88`, `.github/workflows/ci.yml:36`, `README.md:325,385` |
| 2 | `CompiledConnection.bindings` is dropped when creating connection revisions, so edition providers with dependencies (models → storage) cannot deploy | High | `src/commands/deployment_dir.rs:363-373` |
| 3 | Connection revision matching ignores `bindings`, so a binding change reuses the wrong immutable revision | High | `src/commands/deployment_dir.rs:246-258` |
| 4 | Compiled deployment directories cannot express canary rollouts; only hand-written manifests can | High | `orion_api::v2::manifest::CompiledMember` has no `rollouts`; `ManifestMember` does |
| 5 | The edition is invisible in human output; there is no way to require one | High | `src/commands/deployments.rs:159-167` |
| 6 | Edition protocol/provider validation is opt-in and manual; the capabilities file is not tied to the deployment | Medium | `src/commands/authoring/mod.rs:414-457` |
| 7 | Provider dependencies are not validated offline (only operations are) | Medium | `Boundary` in `../Orion/crates/orion-authoring/src/definitions/set.rs:64-75` has `provider_operations` only |
| 8 | Provider list hides tier, revoked state and artifact state | Medium | `src/commands/providers.rs:102-116,153-164` |
| 9 | `provider admit` cannot pass a provider lock | Medium | `src/commands/providers.rs:190-199` uses `AdmitPackageRequest::default()` |
| 10 | `connections list` prints raw JSON even in table mode; bindings are not shown | Medium | `src/commands/connections.rs:105-112` |
| 11 | No edition-aware error hints (`TIER_NOT_ENABLED`, rollout/ref errors) | Low | `src/client.rs:235-252` |
| 12 | No cluster/node view or circuit-breaker command | Medium | only `deployments wait`; paths already exist for breakers |
| 13 | `send` assumes the REST JSON envelope; no protocol-aware guidance or SSE consume path | Medium | `src/commands/data.rs:81-90,128-133` |
| 14 | No edition CI: all tests compose Community only | High | `tests/server/common/mod.rs:37-66` |
| 15 | Capabilities fixtures predate `ProviderBackendInfo.dependencies` and `CompiledConnection.bindings` | Blocker (compile) | `src/commands/v2.rs:54-87,527-534` |

---

## 5. Design principles

1. **One binary.** No edition feature flags, no separate Enterprise CLI, no dependency on `Orion-ext`. Everything edition-specific is discovered over HTTP.
2. **Capabilities are the contract.** The CLI reads drivers, providers, dependencies, policy and edition from `/runtime-capabilities`; it never hardcodes `com.orion.*` ids or edition protocol names.
3. **Preflight, then let the server decide.** Client-side checks catch obvious mismatches before writes; the server remains authoritative for tier limits, rollout partitions and admission.
4. **Offline stays offline.** `lint`, `compile`, `fmt`, `clippy`, `dry-run`, `test`, `sql` and `plugin` never contact a server. Edition knowledge reaches them only through an explicit capabilities file (and, later, a recorded target fingerprint).
5. **Manifest fidelity.** What `compile` writes must survive `apply -d` unchanged — definitions, connections, `bindings`, providers and (when supported) rollouts.
6. **Actionable refusal.** A Community node asked for a canary says "canary rollout requires Standard or Enterprise; this node reports community", not an unexplained 403.

---

## 6. Phases

### Phase 0 — Rebase on Orion v2.0

**Goal:** Build and test against the released v2.0 crates before adding behaviour.

**Work items**

- Move every `orion-*` dependency and the `orion-server` dev-dependency to the v2.0 release commit `15c632a0` (`Cargo.toml:50-88`) — the same commit Orion Ext is cut from. No `v2.0.0` tag exists yet, so the pin is a `rev`; the CLI version stays `2.0.0-rc.1` until Orion's workspace version moves.
- Rename CI's `ORION_TAG` to `ORION_REV` and install the server with `cargo install --rev`; update `README.md`, `CLAUDE.md`, `tests/e2e/README.md` and every install example.
- `cargo update` and commit the rev-based `Cargo.lock` (never a lock produced under the `.cargo/config.toml` patch; generate it with the patch disabled and a local git mirror if the rev is not on GitHub yet).
- Fix the type/API fallout:
  - `ProviderBackendInfo.dependencies` is new in v2.0: update every struct literal/fixture in `src/commands/v2.rs` and the capability fixtures.
  - `CompiledConnection.bindings` is new in v2.0: update the fixtures in `src/commands/deployment_dir.rs` and `tests/server/compiled_deployment_test.rs`.
  - `CreateConnectionRevisionRequest.bindings` already exists in rc.1 but is not populated by the CLI; carrying it is Phase 3, not a compile fix.
- Add `tests/server/api_contract_test.rs`: assert `runtime_capabilities` decodes, `api_version == API_VERSION`, and the typed vocabulary (`EditionTier`, `EntityKind`, `ImplementationKind`) round-trips.

**Acceptance**

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`, `tests/e2e` and `deployments_dir_e2e` are green.
- No `tag = "v2.0.0-rc.1"` or `ORION_TAG` reference remains in the build, CI or docs.
- The `Cargo.lock` orion-* entries carry the git source and rev, not path sources.
- The API-version guard still refuses a server speaking another revision.

---

### Phase 1 — Edition discovery and preflight

**Goal:** Make the target edition and its capabilities a first-class, visible property of every server-facing command.

**Work items**

- `src/commands/deployments.rs` — give `capabilities` a table renderer:
  - Header: `Orion <edition> · v2 admin API <n> · admission policy <id>`.
  - Channel drivers: protocol, sync, async, HTTP route, ingress/hook guards, transport schema present.
  - Provider backends: driver id, kind, short digest, operation count, dependency aliases.
  - Node status: node id, boot id, desired/applied revision, configuration degraded, quarantined entities, last error, backoff, retained revisions.
  - Head revision/epoch and native scheduler in-flight/max.
  - Keep `-o json|yaml` byte-compatible with the server answer; `--quiet` keeps printing just the edition.
- `src/commands/v2.rs` — add one shared `capabilities(client)` helper that checks `api_version` first, then decodes. Use it from `deployments capabilities`, `deployment_dir`, and the preflight.
- `src/main.rs` — add a global `--require-edition <community|standard|enterprise>`:
  - Resolve and compare once per invocation, before dispatch, for commands that contact a server.
  - Local authoring commands are unaffected, and `--require-edition` with one is a clap-level conflict or an explicit no-op warning.
  - Use `EditionTier` ordering (`community < standard < enterprise`), not string equality.
- `src/client.rs` — add hints for `TIER_NOT_ENABLED` ("this node is <edition>; the feature needs standard or enterprise") and for the rollout/manifest refusals. v2.0 exposes it as `orion_api::codes::TIER_NOT_ENABLED` (rc.1 spelled it `ENTERPRISE_TIER_NOT_ENABLED`, so Phase 0 must land first).
- `src/main.rs` long version — print the v2 admin API revision the binary speaks (helpful when filing edition bugs).
- Tests:
  - Table renderer against a Community, a Standard and an Enterprise fixture.
  - Preflight matrix (three server editions × three requested editions).
  - A refused preflight makes no mutating request (mock transport counter).

**Acceptance**

- An operator can answer "which edition am I talking to, and what does it serve?" from one command.
- `--require-edition enterprise` fails before any write, with the actual edition in the message.

---

### Phase 2 — Authoring against an edition

**Goal:** `lint`, `compile` and `clippy` validate edition protocols, provider operations and provider dependencies offline, using a capabilities file.

**Work items**

- Add `provider_backends[].dependencies` parsing to `src/commands/authoring/mod.rs`.
- Validate bindings in the CLI until `orion_authoring::Boundary` grows a dependency field (tracked as an upstream cleanup). `check_provider_bindings` is the one rule and runs after the gate in `lint`, `compile`, `clippy` and `sql check`:
  - each alias is declared by the target provider;
  - every required alias is bound, optional ones may be absent;
  - the bound connection runs on the provider the dependency declares (for example `com.orion.storage` for `artifact_store`);
  - a provider that declares no dependencies cannot have `bindings`.
  A target the capabilities file does not describe is left to the server. The check moves into `orion-authoring` once the boundary carries the declarations.
- Record the target in the deployment directory as a CLI-owned sidecar (`target.json`):
  ```json
  {
    "schema_version": 1,
    "api_version": 1,
    "edition": "enterprise",
    "channel_drivers": ["cron", "http", "rest", "kafka", "sse", "graphql"],
    "provider_digests": { "com.orion.redis": "sha256:..." }
  }
  ```
  `compile --capabilities` writes it; `DeploymentDir::read` treats it as optional (older directories still deploy). A compile without the flag removes a stale one. This file is CLI metadata and is never sent to the server.
- `deployments plan|apply -d` reads the live capabilities once and refuses **before uploading** on:
  - an API revision mismatch recorded in `target.json`;
  - a channel definition whose protocol the target does not serve;
  - a connection provider digest the target does not have.
  An edition downgrade is reported as a warning; the member-by-member checks decide whether the set is actually deployable. A compatible Enterprise-compiled set still applies to Community.
- Document the two-step workflow in README and help:
  ```bash
  orion-cli deployments capabilities -o json > caps.json
  orion-cli compile ./definitions --capabilities caps.json -o dist/deployment
  ```
- Tests:
  - Snippet above compiles a `kafka` channel and a `com.orion.models` connection with a `storage` binding.
  - A set that uses an edition-only protocol or provider is refused before any `POST /definitions`; a compatible Enterprise-compiled set still applies to Community.
  - A directory with no `target.json` keeps the current behaviour.

**Acceptance**

- Edition protocols and providers are caught offline, with field-pathed diagnostics.
- A compiled directory remembers what it was compiled for.

---

### Phase 3 — Deployment manifest fidelity

**Goal:** Everything an edition deployment needs survives the compiled-directory path, and canary rollout is a supported workflow.

**Work items**

- **Bindings (bug fix, no upstream work):**
  - Pass `connection.bindings.clone()` into `CreateConnectionRevisionRequest` (`src/commands/deployment_dir.rs:363-373`).
  - Include `bindings` in `matching_connection_revision` (`:246-258`), so a changed binding creates a new immutable revision instead of reusing the old one.
  - Assert both in `src/commands/v2.rs::apply_dir_assigns_revisions_against_the_head` and in `tests/server/compiled_deployment_test.rs`.
- **Canary via hand-written manifest (available now):**
  - Add client-side validation in `deployments plan|apply -f`: intervals are non-empty, strictly increasing, cover `[0,100)` exactly once, each `workflow_revision` appears once, and every rollout revision is in the channel's `requires`.
  - Render a rollout summary in `plan -f` (`orders: v1 0-90, v2 90-100`).
  - Preflight `edition >= standard`; on Community, refuse locally with a message naming canary and the reported edition.
  - Add an e2e/suites case for a Standard-composed node.
- **Canary via compiled directory (upstream design required):**
  - `CompiledMember` currently names one `definition` digest per id, and `DeploymentDir::read` rejects duplicate `(kind,id)`. A canary needs two workflow revisions of one workflow id, so this cannot be represented without a format change.
  - Proposed upstream change to `orion_api::v2::manifest`:
    ```json
    { "kind": "channel", "id": "orders",
      "rollouts": [
        { "workflow": "orders", "definition": "sha256:...", "start_bucket": 0, "end_bucket": 90 },
        { "workflow": "orders", "definition": "sha256:...", "start_bucket": 90, "end_bucket": 100 }
      ] }
    ```
    Bump `COMPILED_DEPLOYMENT_SCHEMA_VERSION` and keep reading v1. `deployments apply -d` resolves each `definition` digest to a revision, adds every workflow revision to `requires`, and emits `ManifestMember.rollouts`.
  - Alternative if upstream cannot land: a CLI `--rollout channel=workflow@<definition-file>:weight,...` flag on `apply -d`, with the alternate definitions carried under `variants/`. Do not invent this unless the upstream option is rejected; two formats would be worse than one.
- **Uploaded provider packages (only if a deployment carries one):** decide whether `-d` supports a `providers.json` sidecar pinning `ManifestProvider` entries and `owning_package`. Enterprise Rust providers are compiled in, so this is not required for Standard/Enterprise; it is required for workflow/wasm packages. Defer unless a concrete use case arrives.
- Tests:
  - Bindings round-trip compile → apply → `connections get`.
  - Canary manifest accepted on Standard, refused on Community with a CLI-level message.
  - Rollout partition errors caught locally (mirroring `ROLLOUT_PARTITION_INVALID`).
  - If upstream lands, `-d` canary end to end.

**Acceptance**

- A connection with `bindings` deploys correctly and re-deploys only when its configuration or bindings change.
- Canary is a documented, tested path; Community refusal is clear before the request.

---

### Phase 4 — Provider and connection UX for edition backends

**Goal:** An Enterprise operator can see, admit and bind edition providers without reading raw JSON.

**Work items**

- `src/commands/providers.rs`:
  - `list` columns: Publisher, Provider, Version, Kind, **Tier**, **State** (admitted / not admitted / revoked / artifact state), Digest. Keep JSON/YAML untouched.
  - `get` table view (or at least a summary) showing tier, admission policy/fingerprint, artifact state, operations and dependency aliases.
  - `admit --lock <file>` (or `-`): pass `AdmitPackageRequest.lock`. This is how a provider resolution lock is verified.
  - `upload` and `admit`: rely on the API; surface `TIER_NOT_ENABLED` with the Phase 1 hint.
  - `scaffold --tier`: only if `ScaffoldRequest` gains a tier field (upstream API change, which moves `API_VERSION`). Until then, document editing `distribution.json` and keep scaffold's default `community`.
- `src/commands/connections.rs`:
  - `list` table: Connection, Revision, Provider digest (short), Bindings, Configuration digest. JSON/YAML still print the page.
  - `get` human view: provider, public config, secret **locators** (never resolved values), bindings, dependencies, created/updated metadata.
  - `create --bind alias=connection` (repeatable) merged into the request, and `--secret path=env://NAME` (repeatable) for locators. The JSON file remains the full-fidelity path.
  - `probe` table: healthy, latency, probe description, error class, effect status; exit 2 unchanged.
- Tests:
  - Provider list fixture with `tier: standard`, `revoked: true`, `artifact_state`.
  - Admit with a lock file sends the lock.
  - Connections list/get never print secret values; a binding change is visible.
  - Probe failure renders the class/effect.

**Acceptance**

- Tier and revocation are obvious in `provider list`.
- `bindings` are visible and editable from the CLI, not only in JSON.

---

### Phase 5 — Edition channel protocols on the data plane

**Goal:** Sending and consuming through edition channels is either supported or clearly explained.

**Work items**

- `src/commands/data.rs`:
  - Keep the default `{"data": ...}` envelope for REST/HTTP.
  - Document GraphQL over `send --raw`: a JSON body `{"query": "...", "variables": {...}}` to a `graphql` channel. `--raw` already sends the document verbatim; the existing unit test covers that.
  - `--content-type` and repeatable `--header` need a transport hook `orion-client` does not expose. Deferred to the upstream `orion-client` API; JSON is what the GraphQL driver accepts, so nothing edition-specific is blocked.
- `src/client.rs`:
  - Hint on 405 `METHOD_NOT_ALLOWED`: a cron channel has no HTTP route, and an ingress-fed channel such as `kafka` is reached through its transport, not the data plane.
  - Hook validation failures already name the driver's own refusal; no CLI translation added.
- SSE:
  - SSE channels are timer-fed; the useful client action is consuming the stream, not sending.
  - `curl -N` is documented in `send --help` and the README. A `streams tail` command waits on `OrionClient::get_stream` (`impl Stream<Item = Bytes>`), an upstream `orion-client` change.
- Tests:
  - 405 hint unit test.
  - `send --raw` already asserts the payload is sent verbatim; a real GraphQL channel is covered by the optional `orion-ext` e2e job.

**Acceptance**

- A GraphQL channel is usable from the CLI.
- An ingress-fed channel gets an actionable message instead of a bare 405.
- SSE consumption is either implemented or explicitly documented as out of scope.

---

### Phase 6 — Cluster and edition operations

**Goal:** Cluster mode is observable and operable enough to run Standard/Enterprise.

**Work items**

- `deployments wait` / `apply --wait-*`: table view of `ConvergenceStatusResponse` — target revision, converged/pending/degraded node ids, and the local `evaluation` when present. JSON unchanged.
- `deployments capabilities`: node-status section from Phase 1 (node id, boot id, desired/applied, quarantines, last error, backoff, retained revisions). This is the per-node view the API exposes today.
- Add `orion-cli circuit-breakers list|reset <key>`:
  - `GET /api/v2/admin/connectors/circuit-breakers`
  - `POST /api/v2/admin/connectors/circuit-breakers/{key}` (fans out to every node in cluster mode)
  - Table of breaker/key/state, `--quiet` prints keys, `reset` confirms unless `--yes`.
  - Paths already exist in `orion_client::paths`.
- `health`: make the cluster signal explicit — when `components.cluster_store` is present, print it with the other components (already) and add `--require-edition standard` guidance in README for HA installs.
- `backups` in cluster mode: the server refuses; add a hint pointing at managed database snapshots/PITR. Keep the exit code.
- Tests:
  - Convergence table with converged/pending/degraded fixtures.
  - Breaker list/reset against the in-process router; reset fans out (assert the route is called with the key).
  - Cluster-store health rendering.

**Acceptance**

- An operator can tell whether a deployment converged across the fleet and can reset a fleet-wide breaker from the CLI.

---

### Phase 7 — CI, docs, skill and release

**Goal:** Prove both edition paths continuously and document the support matrix.

**Work items**

- Test harness:
  - Add `tests/server/common::test_state_with_edition(EditionTier)` built from `CompositionRoot::community().with_edition(...)`.
  - Add `tests/server/edition_test.rs`:
    - capabilities reports `community` / `standard` / `enterprise`;
    - canary plan/apply succeeds on Standard and is refused `TIER_NOT_ENABLED` on Community;
    - a provider package declaring `tier: "enterprise"` is admitted by an edition-style test verifier and refused by Community's (implement the test verifier with `orion::extension_api::PackageVerifier`, mirroring `ExtPackageVerifier`'s tier rule);
    - preflight `--require-edition` matrix.
  - These run in the existing `test` job; no new job needed.
- CI:
  - Keep the Community e2e/examples jobs unchanged.
  - Add an optional `edition-e2e` job gated on availability of an `orion-ext` binary (private registry/self-hosted runner or a repository secret). It runs `tests/e2e` plus a new `12_editions.sh` suite against `orion-ext`.
  - If private access is unavailable, the composed-node edition tests above still cover capability, tier and canary contracts; the real extension drivers/providers remain covered by `Orion-ext`'s own CI.
  - Add a weekly `orion-head` build job (against Community `main`) as an early warning for API drift, matching `Orion-ext`'s `community-head`.
- Docs:
  - `README.md`: edition matrix, `deployments capabilities` workflow, `--require-edition`, canary via manifest, edition providers/connections, cluster commands, what the CLI cannot do (e.g. sending to a Kafka-fed channel).
  - `CLAUDE.md`: update the architecture notes, the edition model, the new modules/flags, and the test layout.
  - `CHANGELOG.md`: a 2.0.0 section naming edition support, bindings, canary and the version pin.
  - `../Orion/skills/orion/SKILL.md` + `references/cli.md`: teach the agent edition discovery, `--require-edition`, the capabilities-file compile flow, canary manifests and tier admission. This is a cross-repo PR.
  - `../Orion-docs` CLI reference: edition matrix and the new flags (cross-repo PR).
- Release:
  - CLI version `2.0.0`, tag `v2.0.0`; dist/Homebrew/GHCR unchanged.
  - Smoke: `orion-cli --version` names API v2.1 and editions; `deployments capabilities` against Community and Enterprise; `apply -d` of the same set to both (Community refuses edition-only members with the preflight message).

**Acceptance**

- CI proves Community and composed-edition contracts on every PR.
- A real `orion-ext` e2e run is available wherever the artifact is reachable.
- README, skill and docs describe the same support matrix.

---

## 7. Test strategy

| Layer | Community | Standard / Enterprise |
|---|---|---|
| Unit (`src/`) | Existing | Edition comparison, capabilities parsing, hints, table renderers |
| Command flow (`src/commands/v2.rs` mock transport) | Existing | Preflight refusal, bindings round-trip, canary validation, provider tier display |
| Authoring (`tests/authoring/`) | Existing | Capabilities fixture with `kafka`, `sse`, `graphql`, `com.orion.models` dependencies and bindings |
| Server (`tests/server/`) | Existing | `with_edition(Standard/Enterprise)`, canary, tier admission, cluster-store health |
| E2E (`tests/e2e/`) | Existing 11 suites | New `12_editions.sh`, gated on `ORION_EXT_BIN`; canary + capabilities + require-edition |
| Real edition (`orion-ext`) | — | Optional CI job; manual HA drills already exercise the artifact |

Rules to keep:

- Every new list/table keeps the cursor flags and `print_list_footer` behaviour; no `--offset`, no `--sort-by`.
- Every JSON/YAML surface stays the typed API answer; tables are additive.
- A refusal test asserts no mutating request was sent, not only the exit code.
- Exit codes stay meaningful (`probe` 2 unhealthy, `health` 1 degraded, convergence timeout 3).

---

## 8. Cross-repository dependencies

| Dependency | Repo | Needed by | Notes |
|---|---|---|---|
| `v2.0.0` tag | Orion | Phase 0 | Blocking; pin by SHA until tagged |
| `Boundary.provider_dependencies` + `check_bindings` | Orion (`orion-authoring`) | Phase 2 | Keeps binding validation in one place |
| `CompiledMember.rollouts` / schema v2 | Orion (`orion-api`) | Phase 3 canary `-d` | Alternative is a CLI-only flag; prefer upstream |
| `ScaffoldRequest.tier` | Orion (`orion-api`) | Phase 4 optional | Moves `API_VERSION`; only if scaffold must emit a tier |
| `OrionClient::get_stream` | Orion (`orion-client`) | Phase 5 SSE | Optional; docs fallback |
| `ORION_EXT_BIN` / private CI access | Orion-ext | Phase 7 | Optional; composed-node tests remain |
| Skill update | Orion (`skills/orion`) | Phase 7 | Cross-repo PR |
| CLI reference docs | Orion-docs | Phase 7 | Cross-repo PR |

Any upstream request-field addition moves `orion_api::v2::API_VERSION`; the server and CLI must ship together. Budget for that in Phases 2–5.

---

## 9. Risks and decisions

| Risk | Impact | Mitigation |
|---|---|---|
| Orion v2.0 tag not yet published | Phase 0 pins a commit | Pin the release SHA `15c632a0` in every `orion-*` line and `ORION_REV`; switch to the tag in one commit when it exists |
| Upstream API changes slip | Phases 2–3 slip | Sequence the Orion PRs first; keep CLI fallbacks (warning-only binding check, `-f` canary) so v2.0 ships |
| Private `orion-ext` unavailable in public CI | Real edition regression risk | Compose Standard/Enterprise from Community for contract tests; keep optional private e2e; rely on Orion-ext CI for drivers |
| CLI starts hardcoding `com.orion.*` | Drift from Orion-ext | Review rule: only capabilities data and `orion_api` vocabulary in `src/` |
| `target.json` becomes a second format | Confusion | It is CLI metadata only, optional, never sent, and documented as such |
| Too many UX changes at once | Review load | One PR per phase; tables additive; JSON untouched |
| `--require-edition` surprises local commands | Poor UX | Only server-facing commands resolve it; local commands reject the flag explicitly |

**Decisions to make explicit in review**

1. Treat Standard and Enterprise identically in the CLI (feature set), differing only in the reported label and support terms.
2. Canary is first-class via `-f` in v2.0; `-d` canary depends on the upstream compiled-format change.
3. Real `orion-ext` e2e is optional CI, not a release blocker; composed-edition tests are mandatory.
4. SSE consumption is optional; GraphQL sending is supported through `--raw`.

---

## 10. Release checklist

- [ ] All `orion-*` dependencies and `ORION_REV` on the v2.0 release commit (or the `v2.0.0` tag once it exists).
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`, `cargo test --doc`.
- [ ] `tests/e2e` green against the tagged `orion-server`.
- [ ] Composed Standard/Enterprise server tests green.
- [ ] Optional: `ORION_EXT_BIN` e2e green against `orion-ext`.
- [ ] `deployments capabilities` readable on Community and Enterprise; `--require-edition` matrix verified by hand.
- [ ] Bindings round-trip and canary manifest verified on a real server.
- [ ] README/CLAUDE/CHANGELOG/skill/docs updated.
- [ ] `Cargo.lock` has no path sources; no `.cargo/config.toml` patch committed.
- [ ] Tag `v2.0.0`; dist, Homebrew and GHCR publish; `--version` names the API revision and supported editions.

---

## 11. Open questions

1. **Pin:** ~~Is the v2.0.0 tag published on both repositories, or should Phase 0 pin release SHAs?~~ Resolved: no v2 tag exists, so Phase 0 pins Community commit `15c632a0` (the commit Orion Ext is cut from) and CI tracks `ORION_REV`. Move the whole pin to the tag when it is published.
2. **Canary authoring:** Is a hand-written manifest acceptable for v2.0, or is `compile`-produced canary a release requirement (which pulls in the upstream `CompiledMember` change)?
3. **SSE/GraphQL:** Must the CLI consume SSE and send GraphQL in v2.0, or are hints plus `--raw` + documented `curl` enough?
4. **Private CI:** Is there a runner/token that can fetch an `orion-ext` binary for the optional edition e2e job?
5. **Edition flag scope:** Is a global `--require-edition` preferred, or per-command flags on `deployments`/`provider`/`connections` only?
6. **Tier in scaffold:** Is `provider scaffold --tier` needed, or is editing `distribution.json` acceptable?
