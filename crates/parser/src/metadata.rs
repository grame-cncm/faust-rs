//! Compilation-global metadata store for Faust top-level `declare key "value";`.
//!
//! # Source provenance (C++)
//! - `compiler/parser/sourcereader.cpp`
//! - `declareMetadata(Tree key, Tree value)`
//! - `compiler/global.hh` (`gMetaDataSet`)
//!
//! # Mapping status
//! - Semantics: `1:1`
//! - Representation: `adapted`
//!
//! C++ stores top-level `declare` metadata in a process-global map keyed either
//! by the raw metadata key (master document) or by `filename/key` for imported
//! documents. Rust keeps the same session-wide semantics in an explicit shared
//! value owned by the active compilation flow instead of a global singleton.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// Key category for top-level compilation metadata declarations (`declare key "value";`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompilationMetadataKey {
    /// Key emitted by the master document with no file prefix.
    Global { key: Box<str> },
    /// Key emitted by an imported/loaded file and therefore scoped by source.
    Scoped {
        source_file: Box<str>,
        key: Box<str>,
    },
}

impl CompilationMetadataKey {
    /// Returns a master-document key.
    ///
    /// This corresponds to the C++ case where `declare key "value";` is seen in
    /// the entry source itself and therefore contributes directly under `key`
    /// without any filename prefixing.
    #[must_use]
    pub fn global(key: impl Into<Box<str>>) -> Self {
        Self::Global { key: key.into() }
    }

    /// Returns an imported-file-scoped key.
    ///
    /// This corresponds to the C++ rule where metadata emitted by imported
    /// files is keyed under `filename/key` rather than merged into the master's
    /// plain key namespace.
    #[must_use]
    pub fn scoped(source_file: impl Into<Box<str>>, key: impl Into<Box<str>>) -> Self {
        Self::Scoped {
            source_file: source_file.into(),
            key: key.into(),
        }
    }
}

/// Immutable deterministic snapshot of all top-level metadata collected during one parse session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompilationMetadataSnapshot {
    entries: BTreeMap<CompilationMetadataKey, BTreeSet<Box<str>>>,
}

impl CompilationMetadataSnapshot {
    /// Returns the underlying deterministic key/value map.
    ///
    /// Determinism matters for:
    /// - golden snapshots,
    /// - differential parser/eval tests,
    /// - compiler frontends that want a stable serialized view of top-level
    ///   metadata independent from insertion order.
    #[must_use]
    pub fn entries(&self) -> &BTreeMap<CompilationMetadataKey, BTreeSet<Box<str>>> {
        &self.entries
    }
}

#[derive(Debug)]
/// Interior mutable storage backing [`CompilationMetadataStore`] snapshots.
struct CompilationMetadataStoreInner {
    master_source: Box<str>,
    entries: BTreeMap<CompilationMetadataKey, BTreeSet<Box<str>>>,
}

/// Shared compilation-metadata store carried across file imports and evaluation.
///
/// Rust equivalent of the C++ `gMetaDataSet` semantic role. Intentionally shared
/// across parser and evaluator file-loading boundaries so `component(...)` /
/// `library(...)` keep contributing to one compilation-global metadata view.
#[derive(Clone, Debug)]
pub struct CompilationMetadataStore {
    inner: Arc<Mutex<CompilationMetadataStoreInner>>,
}

impl CompilationMetadataStore {
    /// Creates one metadata store bound to the current master source.
    ///
    /// `master_source` defines which file gets the unscoped key treatment. All
    /// other files contributing metadata through the same store are recorded as
    /// imported/scoped entries.
    #[must_use]
    pub fn new(master_source: &str) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CompilationMetadataStoreInner {
                master_source: master_source.into(),
                entries: BTreeMap::new(),
            })),
        }
    }

    /// Records one top-level `declare key "value";` under C++-equivalent scope rules.
    ///
    /// Source provenance (C++):
    /// - `compiler/parser/sourcereader.cpp`
    /// - `declareMetadata(Tree key, Tree value)`
    ///
    /// C++ writes plain `key` in the master document and `filename/key` in
    /// imported documents. Rust preserves that distinction structurally through
    /// [`CompilationMetadataKey`] rather than flattening it into one string.
    ///
    /// Repeated declarations of the same `(key, value)` pair are idempotent:
    /// values are stored in a [`BTreeSet`] so duplicates do not accumulate.
    pub fn declare_top_level(&self, current_source: &str, key: &str, value: &str) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let key = if current_source == guard.master_source.as_ref() {
            CompilationMetadataKey::global(key)
        } else {
            CompilationMetadataKey::scoped(current_source, key)
        };
        guard.entries.entry(key).or_default().insert(value.into());
    }

    /// Records metadata discovered while evaluating a used definition wrapper.
    ///
    /// C++ `eval.cpp` inserts the wrapper's already-qualified key directly in
    /// `gMetaDataSet`. Rust preserves that flattened key as a global entry; the
    /// compiler facade later normalizes its source pathname for backend display.
    pub fn declare_evaluated(&self, key: &str, value: &str) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entries
            .entry(CompilationMetadataKey::global(key))
            .or_default()
            .insert(value.into());
    }

    /// Returns a deterministic snapshot of the currently aggregated metadata.
    ///
    /// The snapshot is a deep clone of the current store contents, so callers
    /// can hold onto it for diagnostics, code generation, or tests without
    /// keeping the store lock alive.
    #[must_use]
    pub fn snapshot(&self) -> CompilationMetadataSnapshot {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        CompilationMetadataSnapshot {
            entries: guard.entries.clone(),
        }
    }
}
