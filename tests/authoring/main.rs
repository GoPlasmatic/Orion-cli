//! The offline authoring tools — `lint`, `compile`, `dry-run`, `test`, `fmt`,
//! `clippy`, `sql check` and `plugin` — driven through the compiled
//! `orion-cli` binary. One test binary: each file is a module declared here.
//! Fixtures are under `tests/fixtures/`: copies of the Orion server crate's
//! fixtures these suites read, and of two example sets. The tests over the
//! whole of Orion's `examples/packages` read an Orion checkout named by
//! `ORION_EXAMPLES_DIR` (see `common::orion_examples`).

// Tests unwrap and panic on what they did not expect; the workspace's
// lints against both are for production code.
#![expect(clippy::unwrap_used, clippy::panic)]

mod common;

mod cli_subcommands_test;
mod clippy_cli_test;
mod clippy_fix_test;
mod compile_test;
mod fmt_cli_test;
mod secrets_offline_test;
mod signing_cli_test;
mod sql_check_cli_test;
mod sql_check_test;
