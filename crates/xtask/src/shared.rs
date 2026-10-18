//! Shared workspace helpers for `xtask` workflows.
//!
//! This module centralizes path handling that is reused across snapshot,
//! report, runtime-trace, and build-artifact commands. Keeping these helpers in
//! one place avoids each workflow re-deriving the workspace root differently.

use super::*;

// ---------------------------------------------------------------------------
// Shared workspace/path helpers
// ---------------------------------------------------------------------------

/// Returns the canonical workspace root path.
pub(crate) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .to_path_buf()
        })
}

/// The reference C++ Faust checkout used by the static report and comparison
/// workflows: `faust`, next to this repository (`../faust`, the layout
/// AGENTS.md describes for the pinned reference).
///
/// Runtime workflows that need a C++ Faust executable use `FAUST_CPP_BIN` or an
/// explicit `--faust-bin` instead.
pub(crate) fn cpp_source_root() -> PathBuf {
    let root = workspace_root();
    root.parent().map_or_else(
        || root.join("..").join("faust"),
        |parent| parent.join("faust"),
    )
}

/// Formats a path relative to the workspace root when possible.
pub(crate) fn workspace_relative_path(path: &Path) -> String {
    let root = workspace_root();
    if let Ok(relative) = path.strip_prefix(&root) {
        return relative.display().to_string();
    }
    if let Ok(canonical) = path.canonicalize()
        && let Ok(relative) = canonical.strip_prefix(&root)
    {
        return relative.display().to_string();
    }
    path.display().to_string()
}
