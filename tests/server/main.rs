//! `orion-cli` against an orion-server node assembled in-process: the tests
//! whose subject is the contract between what this binary writes or signs
//! and what a server accepts. One test binary: each file is a module here.
//!
//! The node is built by `orion::bootstrap::assemble`, the server's own boot
//! path, from the orion-server crate at the tag the other orion-*
//! dependencies name, and driven through `orion::server::build_router`.

// Tests unwrap and panic on what they did not expect; the crate's lints
// against both are for production code.
#![expect(clippy::unwrap_used, clippy::panic)]

mod common;

mod compiled_deployment_test;
mod plugin_signing_test;
