//! Mojo identifiers: the keywords and reserved names, the C++ `snakeCase`
//! rename, and the injective FIR-name to Mojo-identifier map of one namespace.
//! Split from `mod.rs` (file-size threshold); behaviour unchanged.

use std::collections::HashMap;

use super::{CodegenError, CodegenErrorCode};

/// Mojo keywords: never valid as an identifier, in any namespace.
pub(super) const MOJO_KEYWORDS: &[&str] = &[
    "alias", "and", "as", "assert", "async", "await", "borrowed", "break", "case", "class",
    "comptime", "continue", "def", "deinit", "del", "elif", "else", "except", "False", "finally",
    "fn", "for", "from", "global", "if", "import", "in", "inout", "is", "lambda", "match", "mut",
    "None", "nonlocal", "not", "or", "out", "owned", "pass", "raise", "raises", "read", "ref",
    "return", "self", "Self", "struct", "trait", "True", "try", "var", "where", "while", "with",
    "yield",
];

/// Names the emitted code refers to as functions, types or parameters: a local
/// of that name would shadow them.
pub(super) const MOJO_LOCAL_RESERVED: &[&str] = &[
    "abs",
    "acos",
    "acosh",
    "asin",
    "asinh",
    "atan",
    "atan2",
    "atanh",
    "bool",
    "cbrt",
    "ceil",
    "copysign",
    "cos",
    "cosh",
    "dfaust",
    "erf",
    "erfc",
    "exp",
    "exp2",
    "f32",
    "f64",
    "faust_fmod",
    "faust_idiv",
    "faust_irem",
    "faust_round",
    "floor",
    "inf",
    "isfinite",
    "isinf",
    "isnan",
    "log",
    "log10",
    "log2",
    "max",
    "meta",
    "min",
    "mod",
    "nan",
    "pow",
    "print",
    "range",
    "remainder",
    "rint",
    "round",
    "s32",
    "s64",
    "sin",
    "sinh",
    "sqrt",
    "tan",
    "tanh",
    "trunc",
    "u32",
    "u64",
    "ui",
];

/// The C++ `snakeCase` of `__mojo_utils.hh`, character for character.
///
/// Camel case is split with `_`, a run of digits stays together and is
/// followed by `_` before a further character other than the last, and a
/// leading `f_` is removed.
#[must_use]
pub(crate) fn snake_case(src: &str) -> String {
    // The C++ function works on bytes; a non-ASCII name is kept as is rather
    // than having its UTF-8 sequence split.
    if !src.is_ascii() {
        return src.to_owned();
    }
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut res = String::with_capacity(len * 2);
    for i in 0..len {
        let c = bytes[i];
        let is_upper = c.is_ascii_uppercase();
        let is_digit = c.is_ascii_digit();
        let prev = (i > 0).then(|| bytes[i - 1]);
        let prev_is_digit = prev.is_some_and(|p| p.is_ascii_digit());
        let prev_is_lower = prev.is_some_and(|p| p.is_ascii_lowercase());
        let prev_is_upper = prev.is_some_and(|p| p.is_ascii_uppercase());
        let next_is_lower = bytes.get(i + 1).is_some_and(u8::is_ascii_lowercase);
        if prev_is_digit {
            if is_digit {
                res.push(char::from(c));
                continue;
            }
            if i + 1 < len && c != b'_' {
                res.push('_');
            }
        }
        if is_upper {
            if i > 0 && (prev_is_lower || (prev_is_upper && next_is_lower)) {
                res.push('_');
            }
            res.push(char::from(c.to_ascii_lowercase()));
        } else {
            res.push(char::from(c));
        }
    }
    if let Some(rest) = res.strip_prefix("f_") {
        return rest.to_owned();
    }
    res
}

/// An injective FIR-name to Mojo-identifier map for one namespace.
#[derive(Debug, Default)]
pub(super) struct Renamer {
    forward: HashMap<String, String>,
    backward: HashMap<String, String>,
    escape_reserved: bool,
}

impl Renamer {
    pub(super) fn new(escape_reserved: bool) -> Self {
        Self {
            escape_reserved,
            ..Self::default()
        }
    }

    /// Records an identifier that is not a FIR name (`null_val`, a method
    /// parameter): a FIR name renamed onto it is a collision.
    pub(super) fn reserve(&mut self, fir_name: &str, mojo_name: &str) {
        self.forward
            .insert(fir_name.to_owned(), mojo_name.to_owned());
        self.backward
            .insert(mojo_name.to_owned(), fir_name.to_owned());
    }

    pub(super) fn get(&mut self, fir_name: &str) -> Result<String, CodegenError> {
        if let Some(name) = self.forward.get(fir_name) {
            return Ok(name.clone());
        }
        let mut name = snake_case(fir_name);
        if MOJO_KEYWORDS.contains(&name.as_str())
            || (self.escape_reserved && MOJO_LOCAL_RESERVED.contains(&name.as_str()))
        {
            name.push('_');
        }
        if let Some(other) = self.backward.get(&name) {
            return Err(CodegenError::new(
                CodegenErrorCode::NameCollision,
                format!(
                    "FIR names `{other}` and `{fir_name}` both become the Mojo identifier `{name}`"
                ),
            ));
        }
        self.forward.insert(fir_name.to_owned(), name.clone());
        self.backward.insert(name.clone(), fir_name.to_owned());
        Ok(name)
    }
}
