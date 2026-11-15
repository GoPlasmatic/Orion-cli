<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Orion Logo" width="120" height="120">

  # Orion

  **The command-line interface for [Orion](https://github.com/GoPlasmatic/Orion) — deploy services, send data, and inspect traces, schedules and health from your terminal.**

  Deploy a compiled definition set as an immutable revision. Send data through channels. Monitor engine health and metrics. Pair it with the [Orion agent skill](https://github.com/GoPlasmatic/Orion/tree/main/skills/orion) to let an AI assistant drive the same commands.

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.98+-orange.svg)](https://www.rust-lang.org)
  [![GitHub Release](https://img.shields.io/github/v/release/codetiger/Orion-cli)](https://github.com/codetiger/Orion-cli/releases)
</div>

---

## Quick Start

**1. Install the CLI:**

```bash
brew install GoPlasmatic/tap/orion-cli   # or: curl installer, Docker (see Install)
```

**2. Point it at your [Orion server](https://github.com/GoPlasmatic/Orion):**

```bash
orion-cli config set-server http://localhost:8080
```

**3. Check the server is running:**

```bash
orion-cli health
```

```
Orion Server v1.0.0
  Status:       OK
  Uptime:       2h 30m
  Components:
    database     OK
    engine       OK
```

**4. Compile a definition set, deploy it, send data:**

```bash
# Compile a directory of workflow, channel and connector files
orion-cli compile ./definitions -o dist/deployment

# See what would change, then deploy it as the next revision
orion-cli deployments plan -d dist/deployment
orion-cli deployments apply -d dist/deployment

# Send real data through the channel
orion-cli send orders -d '{"order_id":"ORD-9182","total":25000}'
```

A deployment states everything the node serves. Applying a directory replaces
what the previous revision deployed.

---

## Commands

| Command | Description |
|---------|-------------|
| `health` | Check server health and component status |
| `deployments` | Plan, apply, read, wait for and roll back v2 deployments; `-d` deploys a compiled directory |
| `connections` | List, create and probe immutable connection revisions |
| `provider` | Author, lint, pack, upload, admit and revoke connector provider packages |
| `cron` | The scheduled-run ledger — status, list, get, retry or cancel an occurrence |
| `send` | Send data through channels (sync or async; `--profile` for timing breakdown) |
| `traces` | View and monitor execution traces |
| `engine` | View what the engine on this node serves |
| `functions` | Inspect workflow task functions registered in the engine |
| `metrics` | Retrieve Prometheus metrics |
| `audit-logs` | View audit logs of admin actions |
| `backups` | Create and list database backups (SQLite) |
| `cache` | Invalidate a channel response-cache namespace |
| `dlq` | Inspect and requeue the trace dead-letter queue |
| `config` | Configure server URL and defaults |
| `completions` | Generate shell completions (bash, zsh, fish, powershell) |

### Global Flags

```
--server <URL>      Orion server URL (overrides config; env: ORION_SERVER_URL)
--output <FORMAT>   Output format: table, json, yaml (default: table)
--quiet             Suppress output, print only IDs or minimal info
--verbose           Show full response bodies and extra details
--no-color          Disable colored output (env: NO_COLOR)
--yes               Skip confirmation prompts
--require-edition   Refuse a target below community|standard|enterprise
```

---

## Editions

One binary serves Community, Standard and Enterprise. The edition is a
property of the target node, read from
`GET /api/v2/admin/runtime-capabilities`; the CLI never builds one edition's
behaviour into the binary.

```bash
# What this node is, and what it serves: edition, admission policy, channel
# drivers, provider backends, deployment head and reconciler status.
orion-cli deployments capabilities

# Fail before a write when the target is below the edition a change needs.
orion-cli --require-edition enterprise deployments plan -d dist/deployment
```

`--require-edition` is checked once per server-facing command. An offline
command (`lint`, `compile`, `fmt`, `clippy`, `dry-run`, `test`, `sql`,
`plugin`) warns that it has no target to check.

---

## Deployments

`orion-cli compile` writes a definition set as a deployment directory with
no revisions in it. `deployments apply -d` uploads its definitions, reuses or
creates connection revisions, assigns member revisions against the target's
history, plans against the head and applies with the reviewed digest:

```bash
orion-cli deployments plan -d dist/deployment     # what would change
orion-cli deployments apply -d dist/deployment    # commit it as the next revision
orion-cli deployments get <REVISION>              # one committed revision
orion-cli deployments wait <REVISION> --target observed
orion-cli deployments rollback --target-revision 1 --expected-head-revision 3 \
  --idempotency-key rollback-3
```

A directory the head already deploys is not applied again. A rollback commits
an earlier revision's manifest as a new revision.

### Compiling for a specific node

An edition's channel protocols (Kafka, SSE, GraphQL) and connector providers
are known to the node, not to the offline tools. Ask the target for its
capabilities and pass them to `compile`:

```bash
orion-cli deployments capabilities -o json > caps.json
orion-cli compile ./definitions --capabilities caps.json -o dist/deployment
```

`compile` then validates channels against that node's drivers, validates every
connection's `bindings` against the provider's declared dependencies, and
writes a `target.json` beside the deployment recording the edition, API
revision, protocols and provider digests it was built for. `plan|apply -d`
reads that record and the live target before uploading anything: an API
revision mismatch or a channel protocol the target does not serve is refused
before the first write. A directory compiled without `--capabilities` deploys
exactly as before.

### Canary rollout

A channel can split its traffic across workflow revisions by carrying
`rollouts` in a hand-written manifest (`deployments plan|apply -f`). This is a
Standard and Enterprise feature; on a Community node the CLI refuses it before
sending the request:

```json
{
  "entity": { "kind": "channel", "id": "orders-api", "revision": 1 },
  "definition_digest": "sha256:…",
  "requires": [
    { "kind": "workflow", "id": "orders", "revision": 1 },
    { "kind": "workflow", "id": "orders", "revision": 2 }
  ],
  "rollouts": [
    { "workflow_revision": 1, "start_bucket": 0, "end_bucket": 90 },
    { "workflow_revision": 2, "start_bucket": 90, "end_bucket": 100 }
  ]
}
```

The intervals must cover `[0, 100)` exactly once, name each workflow revision
once, and every revision must appear in the channel's `requires`. `plan`
validates those rules locally and prints the split before the request.
A compiled directory names one definition per workflow id, so a canary over
two revisions of one workflow is expressed with a hand-written manifest.

Add `--wait-ms 10000 --wait-target observed` to apply to wait for cluster
convergence. It prints one result containing the apply receipt and the final
`convergence` report. The original `evaluation` describes the receiving node
immediately after commit. A wait timeout retains the result, writes a diagnostic
to stderr and exits 3; `--quiet` prints only the revision. Standalone
`deployments wait` prints the convergence report on success and timeout.

---

## Sending Data

[Processing modes](https://docs.goplasmatic.io/reference/data-api.html) for any workload:

### Synchronous (default)

```bash
orion-cli send orders -d '{"order_id":"ORD-001","amount":150}'

# Include a server-side execution profile (timing breakdown by phase/handler).
# Requires tracing.debug_profile_enabled on the server.
orion-cli send orders -d '{"order_id":"ORD-001","amount":150}' --profile
```

### Asynchronous

```bash
# Fire and forget — returns trace_id
orion-cli send orders --async-mode -d '{"amount":100}'

# Submit and wait for completion
orion-cli send orders --async-mode --wait --timeout 30 -d '{"amount":100}'
```

---

## Scheduled runs

A `protocol: "cron"` channel has no caller, so `send` cannot reach it. Its runs
land in a durable occurrence ledger:

```bash
orion-cli cron status                      # what is scheduled, next fire, last result
orion-cli cron list --status failed
orion-cli cron get <OCCURRENCE_ID>
orion-cli cron retry <OCCURRENCE_ID>       # same occurrence, same scheduled_for
orion-cli cron cancel <OCCURRENCE_ID>      # stop one that has not finished, free its slot
```

A new manual occurrence, now, is `POST /api/v2/admin/channels/{id}/trigger`.

## Traces

View and monitor execution traces:

```bash
# Check trace status
orion-cli traces get <TRACE_ID>

# Poll until complete (with timeout)
orion-cli traces wait <TRACE_ID> --interval 2 --timeout 60
```

Exit codes: `0` completed, `1` failed, `2` timeout.

---

## Engine Status

```bash
# What this node serves — version, uptime, workflow counts, channels, load issues
orion-cli engine status
```

---

## Functions

Inspect the workflow task functions registered in the engine, with their input schemas:

```bash
# List functions (table view)
orion-cli functions list

# Full input schemas as JSON
orion-cli --output json functions list
```

---

## AI Assistants

Install the [Orion agent skill](https://github.com/GoPlasmatic/Orion/tree/main/skills/orion) and an AI coding agent can
drive every command below — authoring workflow JSON, dry-running it, and
compiling and deploying it on its own.

```bash
# from a clone of https://github.com/GoPlasmatic/Orion
mkdir -p .claude/skills && cp -r skills/orion .claude/skills/
```

The skill is knowledge, not a service: the agent acts through this CLI under
your shell, so it inherits exactly your access, every admin write lands in the
audit log under your principal, and nothing new listens on a port.

[Agent Skill Setup](https://docs.goplasmatic.io/ai/skills.html) covers the
machine-wide install, what the skill knows, and how to give an agent its own
scoped credentials. For an assistant with no shell, the
[prompt pack](https://docs.goplasmatic.io/ai/prompt-pack.html) drives the plain
REST API instead.

> **Removed in 1.2.0:** `orion-cli mcp serve`. Its HTTP transport put the full
> admin API on a port with no authentication of its own, and every one of its
> tools mirrored a command this CLI already had. The agent skill covers the same
> ground with a smaller attack surface.

---

## Output Formats

All commands support three output formats:

```bash
orion-cli --output table functions list    # Pretty tables (default)
orion-cli --output json  functions list    # JSON for scripting
orion-cli --output yaml  functions list    # YAML for config files
```

Use `--quiet` for minimal output — ideal for shell scripts.

---

## Configuration

Configuration is stored in `~/.orion/config.toml`:

```toml
server_url = "http://localhost:8080"
default_output = "table"
```

```bash
orion-cli config set-server http://localhost:8080
orion-cli config set default_output json
orion-cli config show
```

**Precedence** (highest to lowest):
1. Command-line flags (`--server`, `--output`)
2. Environment variables (`ORION_SERVER_URL`, `NO_COLOR`)
3. Config file (`~/.orion/config.toml`)

---

## Shell Completions

```bash
# Bash
orion-cli completions bash > ~/.bash_completions/orion-cli

# Zsh
orion-cli completions zsh > ~/.zfunctions/_orion-cli

# Fish
orion-cli completions fish > ~/.config/fish/completions/orion-cli.fish
```

---

## Install

```bash
# Docker
docker run --rm ghcr.io/goplasmatic/orion-cli:latest --server http://host.docker.internal:8080 health

# macOS (Homebrew)
brew install GoPlasmatic/tap/orion-cli

# macOS / Linux / Windows installers: shell and PowerShell one-liners are
# attached to each release:
# https://github.com/codetiger/Orion-cli/releases

# From source
cargo install --git https://github.com/codetiger/Orion-cli orion-cli --locked
```

Verify with `orion-cli --version`. Requires Rust 1.98+ for source builds.

---

## Related

- **[Orion Server](https://github.com/GoPlasmatic/Orion)** — The services runtime platform
- **[API Reference](https://docs.goplasmatic.io/reference/admin-api.html)** — Full REST API documentation
- **[Connectors Guide](https://docs.goplasmatic.io/reference/connectors.html)** — Auth schemes, retry policies, and secrets
- **[CLI Reference](https://docs.goplasmatic.io/reference/cli/index.html)** — Every command and flag
- **[Use Cases & Patterns](https://docs.goplasmatic.io/guides/worked-examples.html)** — Real-world examples and AI prompt templates
- **[Observability](https://docs.goplasmatic.io/operate/monitoring.html)** — Prometheus metrics, health checks, logging

## Contributing

Contributions are welcome! Please open an issue or submit a pull request on [GitHub](https://github.com/codetiger/Orion-cli).

The CLI is built from the Orion Community crates (`orion-api`, `orion-client`,
`orion-authoring`, `orion-engine`, …), declared in `Cargo.toml` as git
dependencies on one Orion release commit:

```toml
orion-api = { git = "https://github.com/GoPlasmatic/Orion", rev = "15c632a099a989ca48e8a3d16ba2beb7901d2a29" }
```

The v2.0 release has no tag yet, so the pin is the release commit Orion Ext
is cut from. Bump that rev on every orion-* line together (the `server` test
binary's `orion-server` dev-dependency included), set `ORION_REV` in
`.github/workflows/ci.yml` to the same commit, and run `cargo update` so
`Cargo.lock` follows. The commit must be reachable on GitHub before a clean
clone (or CI) can build; move the whole pin to the `v2.0.0` tag when it
exists.

```bash
cargo build                                  # Build
cargo test                                   # Run tests
cargo clippy --all-targets -- -D warnings    # Lint (CI treats warnings as errors)
cargo fmt --check                            # Format
```

### Developing against an Orion working tree

To build against local, unreleased Orion changes, add a `.cargo/config.toml`
(gitignored) that patches the git source onto your checkout:

```toml
[patch."https://github.com/GoPlasmatic/Orion"]
orion-api = { path = "/path/to/Orion/crates/orion-api" }
orion-authoring = { path = "/path/to/Orion/crates/orion-authoring" }
orion-client = { path = "/path/to/Orion/crates/orion-client" }
orion-common = { path = "/path/to/Orion/crates/orion-common" }
orion-definitions = { path = "/path/to/Orion/crates/orion-definitions" }
orion-engine = { path = "/path/to/Orion/crates/orion-engine" }
orion-plugin-host = { path = "/path/to/Orion/crates/orion-plugin-host" }
orion-runtime = { path = "/path/to/Orion/crates/orion-runtime" }
orion-storage = { path = "/path/to/Orion/crates/orion-storage" }
orion-server = { path = "/path/to/Orion/crates/orion-server" }
```

Two things to know:

- Cargo still fetches the git source named in `Cargo.toml` before applying
  the patch, so the rev must be reachable — on GitHub, or through a local
  mirror mapped with git's `url.<base>.insteadOf` (and
  `CARGO_NET_GIT_FETCH_WITH_CLI=true`).
- With the patch active, cargo rewrites the orion-* entries in `Cargo.lock`
  to path sources. Do not commit that: `git checkout Cargo.lock` before
  committing (CI builds with `--locked` against the rev).

### Tests

- `tests/authoring/` — the offline tools, through the binary. Fixtures are
  under `tests/fixtures/`. The tests over all of Orion's example packages
  read an Orion checkout's `examples/` from `ORION_EXAMPLES_DIR` and skip
  without it; the Postgres/MySQL half of `sql_check_test` is `#[ignore]`d
  and needs Docker (`cargo test --test authoring -- --ignored sql_check_test`).
- `tests/server/` — the CLI against an orion-server node assembled
  in-process (the `orion-server` dev-dependency).
- `tests/deployments_dir_e2e.rs` — `compile` → `deployments apply -d` against
  a real `orion-server` process named by `ORION_BIN` (skips without it).
- `tests/e2e/` — the shell end-to-end suite against a real server; see
  [its README](tests/e2e/README.md):

```bash
cargo install --git https://github.com/GoPlasmatic/Orion --rev 15c632a099a989ca48e8a3d16ba2beb7901d2a29 orion-server --locked
ORION_EXAMPLES_DIR=/path/to/Orion/examples ./tests/e2e/run.sh
```

## License

Apache-2.0 — see [LICENSE](LICENSE) for details.

---

If Orion CLI is useful to you, a ⭐ on [GitHub](https://github.com/codetiger/Orion-cli) helps other developers find it.
