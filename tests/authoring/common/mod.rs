//! Helpers shared by the authoring-tool tests.

/// The compiled `orion-cli` binary. `env!` rather than a runtime lookup:
/// cargo guarantees `CARGO_BIN_EXE_<name>` at compile time for an
/// integration test of the crate that owns the binary.
pub fn orion_bin() -> String {
    env!("CARGO_BIN_EXE_orion-cli").to_string()
}

/// `packages/` of the Orion examples directory `ORION_EXAMPLES_DIR` names
/// (an Orion checkout's `examples/`, at the tag the orion-* dependencies
/// name). The examples are Orion's, not this repository's, so a test over
/// all of them says it skipped when none is named; CI always names one.
pub fn orion_examples() -> Option<std::path::PathBuf> {
    match std::env::var_os("ORION_EXAMPLES_DIR") {
        Some(dir) => {
            let packages = std::path::Path::new(&dir).join("packages");
            assert!(
                packages.is_dir(),
                "ORION_EXAMPLES_DIR has no packages/: {}",
                packages.display()
            );
            Some(packages)
        }
        None => {
            eprintln!("skipped: set ORION_EXAMPLES_DIR to an Orion checkout's examples/");
            None
        }
    }
}

/// Self-cleaning scratch directory, unique per call even when tests run in
/// parallel within one microsecond.
pub struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    pub fn new(label: &str) -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "orion_cli_test_{label}_{}_{}_{}",
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
