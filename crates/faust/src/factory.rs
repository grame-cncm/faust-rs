//! A compiled program.

use std::path::Path;
use std::sync::Arc;

use crate::backend::{RawFactory, Source};
use crate::dsp::Dsp;
use crate::{Backend, CompileOptions, Error, ErrorKind, Precision};

/// What a [`Factory`] and its instances share: the backend's factory pointer
/// and the options it was compiled with.
pub(crate) struct FactoryInner {
    pub(crate) raw: RawFactory,
    pub(crate) precision: Precision,
    pub(crate) name: String,
}

// SAFETY: the pointer is a reference into the backend's factory cache; every
// lifecycle operation on it goes through the crate's process-wide lock, and
// the pointer is never handed out.
unsafe impl Send for FactoryInner {}
unsafe impl Sync for FactoryInner {}

impl Drop for FactoryInner {
    fn drop(&mut self) {
        // Every instance holds an `Arc` of this, so none is alive here.
        self.raw.delete();
    }
}

/// A compiled Faust program: the code its instances run, its arities, its
/// description. Cheap to clone; a clone is another handle on the same program.
#[derive(Clone)]
pub struct Factory {
    pub(crate) inner: Arc<FactoryInner>,
}

impl Factory {
    /// Compiles the program of a file. Its directory is searched by
    /// `import(...)` before `options.import_dirs`.
    pub fn from_file(path: impl AsRef<Path>, options: &CompileOptions) -> Result<Self, Error> {
        let path = path.as_ref();
        let text = path
            .to_str()
            .ok_or_else(|| Error::new(ErrorKind::Compile, "the path is not UTF-8"))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "dsp".to_owned());
        Self::build(Source::File(text), name, options)
    }

    /// Compiles `source`; `name` names the program (the root group of its
    /// controls) and its error messages.
    pub fn from_source(name: &str, source: &str, options: &CompileOptions) -> Result<Self, Error> {
        Self::build(Source::Text { name, source }, name.to_owned(), options)
    }

    fn build(source: Source<'_>, name: String, options: &CompileOptions) -> Result<Self, Error> {
        let raw = RawFactory::create(options.backend, &source, &options.argv(), options.opt_level)?;
        Ok(Self {
            inner: Arc::new(FactoryInner {
                raw,
                precision: options.precision,
                name,
            }),
        })
    }

    pub fn backend(&self) -> Backend {
        self.inner.raw.backend()
    }

    pub fn precision(&self) -> Precision {
        self.inner.precision
    }

    /// The name given at compilation: the file stem, or the `name` of
    /// [`Factory::from_source`].
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// The JSON description of the program: its UI tree and metadata, as the
    /// C API's `getDSPFactoryJSON` returns it.
    pub fn json(&self) -> String {
        self.inner.raw.json()
    }

    /// Creates an instance, initialised at `sample_rate`, its controls at
    /// their initial values.
    pub fn instantiate(&self, sample_rate: i32) -> Result<Dsp, Error> {
        Dsp::create(Arc::clone(&self.inner), sample_rate)
    }
}

impl std::fmt::Debug for Factory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Factory")
            .field("name", &self.inner.name)
            .field("backend", &self.backend())
            .field("precision", &self.inner.precision)
            .finish()
    }
}
