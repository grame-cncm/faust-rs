//! Generates static parity matrices for the libfaust Box and Signal C APIs.
//!
//! The command intentionally starts with a conservative source scan: it parses
//! `LIBFAUST_API` prototypes from the C reference headers and compares their C
//! symbol names with Rust `extern "C"` functions currently present in the FFI
//! crates. It does not infer semantic parity; manual notes keep known adapted
//! mappings visible until dedicated implementation work closes them.

use super::*;

const DEFAULT_OUT_DIR: &str = "porting/generated";

#[derive(Debug, Clone, Eq, PartialEq)]
struct CApiSymbol {
    name: String,
    signature: String,
}

/// Generates Box and Signal API parity matrices from the local C++ Faust tree.
pub(crate) fn libfaust_api_matrix(
    args: LibfaustApiMatrixArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let cpp_root = args
        .cpp_root
        .unwrap_or_else(|| PathBuf::from(CPP_SOURCE_ROOT));
    let out_dir = args
        .out_dir
        .map(|path| workspace_root().join(path))
        .unwrap_or_else(|| workspace_root().join(DEFAULT_OUT_DIR));

    let header_dir = cpp_root.join("architecture/faust/dsp");
    let box_header = header_dir.join("libfaust-box-c.h");
    let signal_header = header_dir.join("libfaust-signal-c.h");
    if !box_header.exists() {
        return Err(format!("missing Box C header: {}", box_header.display()).into());
    }
    if !signal_header.exists() {
        return Err(format!("missing Signal C header: {}", signal_header.display()).into());
    }

    fs::create_dir_all(&out_dir)?;

    let rust_exports = collect_rust_c_exports(&workspace_root().join("crates"))?;
    let box_symbols = parse_libfaust_api_symbols(&fs::read_to_string(&box_header)?);
    let signal_symbols = parse_libfaust_api_symbols(&fs::read_to_string(&signal_header)?);

    write_matrix(
        &out_dir.join("libfaust-box-c-api-matrix.md"),
        "libfaust Box C API Matrix",
        &box_header,
        &box_symbols,
        &rust_exports,
    )?;
    write_matrix(
        &out_dir.join("libfaust-signal-c-api-matrix.md"),
        "libfaust Signal C API Matrix",
        &signal_header,
        &signal_symbols,
        &rust_exports,
    )?;

    println!(
        "wrote {} and {}",
        workspace_relative_path(&out_dir.join("libfaust-box-c-api-matrix.md")),
        workspace_relative_path(&out_dir.join("libfaust-signal-c-api-matrix.md"))
    );
    Ok(())
}

fn parse_libfaust_api_symbols(header: &str) -> Vec<CApiSymbol> {
    let mut symbols = Vec::new();
    let mut pending = String::new();

    for raw_line in header.lines() {
        let line = raw_line.trim();
        if line.starts_with("//") || line.starts_with('*') || line.is_empty() {
            continue;
        }
        if pending.is_empty() {
            if !line.contains("LIBFAUST_API") {
                continue;
            }
            pending.push_str(line);
        } else {
            pending.push(' ');
            pending.push_str(line);
        }

        if pending.ends_with(';') {
            if let Some(name) = extract_c_function_name(&pending) {
                symbols.push(CApiSymbol {
                    name,
                    signature: pending.trim_end_matches(';').to_string(),
                });
            }
            pending.clear();
        }
    }

    symbols.sort_by(|a, b| a.name.cmp(&b.name));
    symbols.dedup_by(|a, b| a.name == b.name);
    symbols
}

pub(crate) fn extract_c_function_name(signature: &str) -> Option<String> {
    let before_paren = signature.split_once('(')?.0;
    before_paren
        .split_whitespace()
        .last()
        .map(|name| name.trim_start_matches('*').to_string())
        .filter(|name| !name.is_empty())
}

fn collect_rust_c_exports(crates_dir: &Path) -> io::Result<BTreeMap<String, Vec<String>>> {
    let mut exports = BTreeMap::new();
    for file in rust_ffi_source_files(crates_dir)? {
        let source = fs::read_to_string(&file)?;
        for name in parse_rust_exported_c_symbols(&source) {
            exports
                .entry(name)
                .or_insert_with(Vec::new)
                .push(workspace_relative_path(&file));
        }
    }
    Ok(exports)
}

fn rust_ffi_source_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let path_text = path.to_string_lossy();
                if path_text.contains("-ffi/src/") {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

fn parse_rust_exported_c_symbols(source: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for line in source.lines() {
        let line = line.trim();
        if let Some(after_fn) = line.split("extern \"C\" fn ").nth(1)
            && let Some((name, _)) = after_fn.split_once('(')
        {
            names.insert(name.trim().to_string());
        }

        if line.contains("!(") {
            for token in c_symbol_tokens(line) {
                names.insert(token);
            }
        }
    }
    names.into_iter().collect()
}

fn c_symbol_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            if is_c_api_symbol_name(&current) {
                tokens.push(current.clone());
            }
            current.clear();
        }
    }
    if !current.is_empty() && is_c_api_symbol_name(&current) {
        tokens.push(current);
    }
    tokens
}

fn is_c_api_symbol_name(token: &str) -> bool {
    token.starts_with('C')
        && token
            .chars()
            .nth(1)
            .is_some_and(|ch| ch.is_ascii_lowercase() || ch == 'D')
}

fn write_matrix(
    path: &Path,
    title: &str,
    header_path: &Path,
    symbols: &[CApiSymbol],
    rust_exports: &BTreeMap<String, Vec<String>>,
) -> io::Result<()> {
    let mut out = String::new();
    writeln!(out, "# {title}").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "Generated by `cargo run -p xtask -- libfaust-api-matrix`."
    )
    .unwrap();
    writeln!(out, "Reference header: `{}`.", header_path.display()).unwrap();
    writeln!(out).unwrap();
    writeln!(out, "| C symbol | Rust implementation | Status | Notes |").unwrap();
    writeln!(out, "| --- | --- | --- | --- |").unwrap();

    for symbol in symbols {
        let implementation = rust_exports
            .get(&symbol.name)
            .map(|paths| paths.join("<br>"))
            .unwrap_or_else(|| "-".to_string());
        let status = classify_symbol(&symbol.name, rust_exports.contains_key(&symbol.name));
        let notes = manual_note(&symbol.name).unwrap_or("-");
        writeln!(
            out,
            "| `{}` | {} | `{}` | {} |",
            symbol.name, implementation, status, notes
        )
        .unwrap();
    }

    fs::write(path, out)
}

fn classify_symbol(name: &str, implemented: bool) -> &'static str {
    if implemented {
        if manual_note(name).is_some() {
            "implemented-nearest-rust-ir"
        } else {
            "implemented-exact-candidate"
        }
    } else {
        "missing"
    }
}

fn manual_note(name: &str) -> Option<&'static str> {
    match name {
        "CisBoxPrim0" | "CisBoxPrim1" | "CisBoxPrim2" | "CisBoxPrim3" | "CisBoxPrim4"
        | "CisBoxPrim5" => Some(
            "Plan note: primitive function pointer identity is approximated by Rust IR shape until exact metadata exists.",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_and_multiline_libfaust_api_prototypes() {
        let header = r#"
            LIBFAUST_API Box CboxInt(int n);
            LIBFAUST_API Signal* CboxesToSignals(
                Box box,
                char* error_msg
            );
        "#;

        let symbols = parse_libfaust_api_symbols(header);

        assert_eq!(
            symbols,
            vec![
                CApiSymbol {
                    name: "CboxInt".to_string(),
                    signature: "LIBFAUST_API Box CboxInt(int n)".to_string(),
                },
                CApiSymbol {
                    name: "CboxesToSignals".to_string(),
                    signature: "LIBFAUST_API Signal* CboxesToSignals( Box box, char* error_msg )"
                        .to_string(),
                },
            ]
        );
    }

    #[test]
    fn parses_rust_extern_c_function_names() {
        let source = r#"
            pub extern "C" fn CboxInt(n: c_int) -> *mut c_void { todo!() }
            pub unsafe extern "C" fn CboxesToSignals(
                box_root: *mut c_void,
                error_msg: *mut c_char,
            ) -> *mut *mut c_void { todo!() }
        "#;

        let names = parse_rust_exported_c_symbols(source);

        assert_eq!(names, vec!["CboxInt", "CboxesToSignals"]);
    }

    #[test]
    fn parses_macro_generated_c_symbol_names() {
        let source = r#"
            prim0!(CboxWire, wire);
            binop!(CboxAdd, CboxAddAux, add);
            match_unary_out!(CisBoxButton, BoxMatch::Button);
        "#;

        let names = parse_rust_exported_c_symbols(source);

        assert_eq!(
            names,
            vec!["CboxAdd", "CboxAddAux", "CboxWire", "CisBoxButton"]
        );
    }
}
