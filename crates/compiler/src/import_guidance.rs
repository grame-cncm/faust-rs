//! Guidance attached to an unresolved import (`FRS-SRC-0002`).
//!
//! The source reader owns the facts of the failure: the name, the importing
//! file and the directories searched. This module adds the way out a writer
//! most often needs, which the reader has no reason to know about: the standard
//! libraries all come from `import("stdfaust.lib");`, each under its prefix,
//! and an invented name (`mathfaust.lib`, `mooreverb.lib`) usually means one of
//! them.
//!
//! Nothing here looks at the file system: the guidance does not say whether a
//! file exists anywhere, only what the name looks like.

use super::*;

/// The libraries `stdfaust.lib` binds, with their prefixes, as in
/// `libraries/stdfaust.lib` of the pinned C++ reference (`8eebea429`).
const STANDARD_LIBRARIES: &[(&str, &str)] = &[
    ("aa", "aanl.lib"),
    ("sf", "all.lib"),
    ("an", "analyzers.lib"),
    ("ba", "basics.lib"),
    ("co", "compressors.lib"),
    ("db", "debug.lib"),
    ("de", "delays.lib"),
    ("dm", "demos.lib"),
    ("dx", "dx7.lib"),
    ("en", "envelopes.lib"),
    ("fd", "fds.lib"),
    ("fi", "filters.lib"),
    ("ho", "hoa.lib"),
    ("hy", "hysteresis.lib"),
    ("it", "interpolators.lib"),
    ("la", "linearalgebra.lib"),
    ("ma", "maths.lib"),
    ("mi", "mi.lib"),
    ("mo", "motion.lib"),
    ("ef", "misceffects.lib"),
    ("no", "noises.lib"),
    ("os", "oscillators.lib"),
    ("pf", "phaflangers.lib"),
    ("pl", "platform.lib"),
    ("pm", "physmodels.lib"),
    ("qu", "quantizers.lib"),
    ("rm", "reducemaps.lib"),
    ("re", "reverbs.lib"),
    ("ro", "routes.lib"),
    ("sp", "spats.lib"),
    ("si", "signals.lib"),
    ("so", "soundfiles.lib"),
    ("sy", "synths.lib"),
    ("ve", "vaeffects.lib"),
    ("vl", "version.lib"),
    ("wa", "webaudio.lib"),
    ("wd", "wdmodels.lib"),
];

/// Names a writer gives the bundle itself.
const BUNDLE_NAMES: &[&str] = &[
    "std",
    "stdlib",
    "stdlibs",
    "stdfaust",
    "faust",
    "faustlib",
    "faustlibs",
    "standard",
];

/// What an unresolved import name looks like.
#[derive(Debug, PartialEq, Eq)]
enum Meant {
    /// `stdfaust.lib` itself (`std.dsp`, `faust.lib`).
    Bundle,
    /// One standard library: its prefix and file.
    Library(&'static str, &'static str),
}

/// Adds the standard-library guidance to an unresolved import.
///
/// Every name but `stdfaust.lib` gets a note saying that one import gives all
/// the standard libraries; a name that looks like one of them, or like the
/// bundle, gets a help naming it, first, ahead of the search-path help. When
/// `stdfaust.lib` itself is not found, the libraries are missing from the
/// search path, and only the search-path help is true.
pub(crate) fn add_import_guidance(
    mut diagnostic: Diagnostic,
    error: &SourceReaderError,
) -> Diagnostic {
    let SourceReaderError::UnresolvedImport { name, .. } = error else {
        return diagnostic;
    };
    let file = file_name(name);
    if file == "stdfaust.lib" {
        return diagnostic;
    }
    diagnostic = diagnostic.with_note(
        "`import(\"stdfaust.lib\");` gives every standard library, each under its prefix: `ma.` for maths.lib, `fi.` filters.lib, `de.` delays.lib, `os.` oscillators.lib, `en.` envelopes.lib, `si.` signals.lib, `ba.` basics.lib, `re.` reverbs.lib, …",
    );
    let help = match standard_library_meant(file) {
        Some(Meant::Bundle) => {
            format!("`{name}` looks like `stdfaust.lib`, which imports every standard library")
        }
        Some(Meant::Library(prefix, library)) if library == file => format!(
            "`{library}` is a standard library: after `import(\"stdfaust.lib\");`, its functions are `{prefix}.<name>`"
        ),
        Some(Meant::Library(prefix, library)) => format!(
            "`{name}` looks like the standard `{library}`: after `import(\"stdfaust.lib\");`, its functions are `{prefix}.<name>`"
        ),
        None => return diagnostic,
    };
    diagnostic.help.insert(0, help.into());
    diagnostic
}

/// The last component of an import name, whichever separator it uses.
fn file_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// The standard library, or the bundle, a file name looks like, when exactly
/// one fits.
///
/// The stem is read without case, extension or punctuation, then without a
/// `faust` or `lib` it carries (`mathfaust` is `math`). It fits a library when
/// it is its stem (`filters`), its stem without the plural (`filter`), its
/// prefix (`fi`), or contains that stem without the plural, if four letters or
/// more (`mooreverb` contains `reverb`).
fn standard_library_meant(file: &str) -> Option<Meant> {
    let stem: String = file
        .split('.')
        .next()
        .unwrap_or(file)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if BUNDLE_NAMES.contains(&stem.as_str()) {
        return Some(Meant::Bundle);
    }
    let bare = stem.replace("faust", "").replace("lib", "");
    let root = |library: &str| {
        let library_stem = library.trim_end_matches(".lib");
        library_stem
            .strip_suffix('s')
            .unwrap_or(library_stem)
            .to_owned()
    };
    for candidate in [stem.as_str(), bare.as_str()] {
        if candidate.is_empty() {
            continue;
        }
        let exact = STANDARD_LIBRARIES.iter().find(|(prefix, library)| {
            candidate == *prefix
                || candidate == library.trim_end_matches(".lib")
                || candidate == root(library)
        });
        if let Some((prefix, library)) = exact {
            return Some(Meant::Library(prefix, library));
        }
    }
    let mut contained = STANDARD_LIBRARIES.iter().filter(|(_, library)| {
        let root = root(library);
        root.len() >= 4 && bare.contains(&root)
    });
    match (contained.next(), contained.next()) {
        (Some((prefix, library)), None) => Some(Meant::Library(prefix, library)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Meant, add_import_guidance, standard_library_meant};
    use crate::SourceReaderError;
    use diagnostics::{Diagnostic, Severity, Stage, codes};

    #[test]
    fn a_name_is_read_as_the_library_it_looks_like() {
        let meant = |file| standard_library_meant(file);
        assert_eq!(
            meant("mathfaust.lib"),
            Some(Meant::Library("ma", "maths.lib"))
        );
        assert_eq!(
            meant("mooreverb.lib"),
            Some(Meant::Library("re", "reverbs.lib"))
        );
        assert_eq!(
            meant("filter.lib"),
            Some(Meant::Library("fi", "filters.lib"))
        );
        assert_eq!(
            meant("filters.lib"),
            Some(Meant::Library("fi", "filters.lib"))
        );
        assert_eq!(
            meant("os.lib"),
            Some(Meant::Library("os", "oscillators.lib"))
        );
        assert_eq!(meant("std.dsp"), Some(Meant::Bundle));
        assert_eq!(meant("faust.lib"), Some(Meant::Bundle));
        assert_eq!(meant("rt.lib"), None);
        assert_eq!(meant("PolyBLEP.h"), None);
        assert_eq!(meant("mylib.lib"), None);
    }

    /// When `stdfaust.lib` itself is not found, pointing at it says nothing
    /// true: the libraries are missing from the search path.
    #[test]
    fn a_missing_stdfaust_gets_no_note() {
        let guided = |name: &str| {
            let error = SourceReaderError::UnresolvedImport {
                name: name.into(),
                from: "program.dsp".into(),
                site: None,
                searched: Vec::new(),
            };
            let diagnostic = Diagnostic::new(
                Severity::Error,
                Stage::SourceReader,
                codes::SRC_UNRESOLVED_IMPORT,
                "cannot resolve import",
            );
            add_import_guidance(diagnostic, &error)
        };
        assert!(guided("stdfaust.lib").notes.is_empty());
        assert!(guided("libs/stdfaust.lib").notes.is_empty());
        assert_eq!(guided("filters.lib").notes.len(), 1);
        assert_eq!(
            guided("filters.lib").help.first().map(AsRef::as_ref),
            Some(
                "`filters.lib` is a standard library: after `import(\"stdfaust.lib\");`, its functions are `fi.<name>`"
            )
        );
    }
}
