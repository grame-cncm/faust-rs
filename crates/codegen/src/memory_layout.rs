//! Custom memory-manager mode and, in later phases, canonical allocation
//! analysis shared by native backends.
//!
//! # Source provenance
//!
//! The option mapping follows Faust C++ `compiler/global.cpp`, where `-mem`,
//! `-mem0`, `--memory-manager`, and `--memory-manager0` all select
//! `gMemoryManager = 0`. Unlike that process-global integer, faust-rs passes a
//! typed value explicitly through each compilation request and backend option.
//!
//! API mapping status: `adapted`. Only mode zero is in the approved Rust port;
//! future C++ modes are deliberately absent so they cannot be accepted and
//! silently lowered as [`MemoryManagerMode::Mem0`].

/// Native backend custom-memory allocation strategy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryManagerMode {
    /// Preserve the backend's ordinary embedded/owned state layout.
    #[default]
    None,
    /// Externalize eligible DSP arrays and runtime-generated tables through
    /// the host memory-manager contract.
    Mem0,
}

impl MemoryManagerMode {
    /// Canonical Faust option spelling recorded in generated metadata and
    /// factory identities.
    #[must_use]
    pub const fn option_spelling(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Mem0 => Some("-mem0"),
        }
    }

    /// Whether custom allocation analysis and emission are enabled.
    #[must_use]
    pub const fn is_mem0(self) -> bool {
        matches!(self, Self::Mem0)
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryManagerMode;

    #[test]
    fn default_is_ordinary_embedded_memory() {
        assert_eq!(MemoryManagerMode::default(), MemoryManagerMode::None);
        assert_eq!(MemoryManagerMode::None.option_spelling(), None);
        assert!(!MemoryManagerMode::None.is_mem0());
    }

    #[test]
    fn mem0_has_one_canonical_spelling() {
        assert_eq!(MemoryManagerMode::Mem0.option_spelling(), Some("-mem0"));
        assert!(MemoryManagerMode::Mem0.is_mem0());
    }
}
