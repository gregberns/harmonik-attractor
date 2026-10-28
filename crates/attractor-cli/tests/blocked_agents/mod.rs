//! Fail-loud stubs for the agent CLIs, first on `PATH`, so a test can never
//! reach a real agent on the user's `PATH`. `fake_agent` uses it, so a test
//! binary with `mod fake_agent;` also declares `mod blocked_agents;`.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Agent CLIs a test must never reach on the user's `PATH`: each gets a
/// stub that exits 2 unless a test shims a fake in its place.
pub const BLOCKED_AGENTS: [&str; 4] = ["claude", "codex", "gemini", "pi"];

/// Writes `bin/<agent>` for every [`BLOCKED_AGENTS`] entry: a stub that
/// refuses to run.
pub fn block_agents(bin: &Path) {
    fs::create_dir_all(bin).unwrap();
    for agent in BLOCKED_AGENTS {
        let stub = bin.join(agent);
        fs::write(
            &stub,
            format!(
                "#!/bin/sh\necho 'real agent blocked in tests: call shim_{agent}_on_path()' >&2\nexit 2\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// The test's `PATH` with `bin` first.
pub fn path_with(bin: &Path) -> OsString {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<PathBuf> = vec![bin.to_path_buf()];
    paths.extend(std::env::split_paths(&path));
    std::env::join_paths(paths).unwrap()
}
