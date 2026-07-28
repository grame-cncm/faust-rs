//! Builds and verifies the local libfaust-rs C/C++ distribution surface.
//!
//! The check intentionally exercises the installed shape of the Rust port:
//! build the unified `faust-ffi` dynamic library, compare exported symbols
//! against a checked-in baseline and maintained C headers, and syntax-check
//! tiny C and C++ clients with callback-table layout assertions.

use super::*;

const EXPORT_BASELINE_REL_PATH: &str = "porting/generated/libfaust-rs-exported-symbols.txt";

/// Builds `faust-ffi`, publishes the native `libfaust-rs` artifacts, checks
/// exported C symbols against the checked-in baseline and local headers, and
/// syntax-checks tiny C/C++ clients using the maintained wrapper headers,
/// including `UIGlue`, `MetaGlue`, and `FAUSTFLOAT` layout assertions.
///
/// `--bless` refreshes the exported-symbol baseline after the header coverage
/// check succeeds. Baseline refreshes are explicit because removing an export
/// is an external ABI change even when no Rust caller observes it.
pub(crate) fn libfaust_export_check(
    args: LibfaustExportCheckArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let dynamic_library = build_libfaust_distribution(false)?;

    let workspace = workspace_root();
    if !dynamic_library.exists() {
        return Err(format!(
            "expected libfaust-rs dynamic library after build: {}",
            dynamic_library.display()
        )
        .into());
    }

    let header_symbols = expected_header_symbols(&workspace)?;
    let exported = exported_dynamic_symbols(&dynamic_library)?;
    let missing = header_symbols
        .difference(&exported)
        .cloned()
        .collect::<Vec<String>>();
    if !missing.is_empty() {
        return Err(format!(
            "libfaust-rs dynamic library is missing header-declared exports: {}",
            missing.join(", ")
        )
        .into());
    }

    let baseline_path = workspace.join(EXPORT_BASELINE_REL_PATH);
    if args.bless {
        write_export_baseline(&baseline_path, &exported)?;
        let non_header_exports = exported
            .difference(&header_symbols)
            .cloned()
            .collect::<Vec<String>>();
        println!(
            "non-header exports captured by the baseline ({}): {}",
            non_header_exports.len(),
            display_symbol_diff(&non_header_exports)
        );
    } else {
        let baseline = read_export_baseline(&baseline_path)?;
        let removed = baseline
            .difference(&exported)
            .cloned()
            .collect::<Vec<String>>();
        let added = exported
            .difference(&baseline)
            .cloned()
            .collect::<Vec<String>>();
        if !removed.is_empty() || !added.is_empty() {
            return Err(format!(
                "libfaust-rs exports differ from {}:\nremoved: {}\nadded: {}\nrefresh intentionally with `cargo run -p xtask -- libfaust-export-check --bless`",
                EXPORT_BASELINE_REL_PATH,
                display_symbol_diff(&removed),
                display_symbol_diff(&added)
            )
            .into());
        }
    }

    syntax_check_headers(&workspace)?;

    println!(
        "libfaust-rs export check: {} exports, {} header declarations, baseline {}{}",
        exported.len(),
        header_symbols.len(),
        EXPORT_BASELINE_REL_PATH,
        if args.bless { " refreshed" } else { " matched" }
    );
    println!(
        "libfaust-rs export artifact: {}",
        workspace_relative_path(&dynamic_library)
    );
    Ok(())
}

fn display_symbol_diff(symbols: &[String]) -> String {
    if symbols.is_empty() {
        "(none)".to_owned()
    } else {
        symbols.join(", ")
    }
}

fn read_export_baseline(path: &Path) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(path).map_err(|error| {
        format!(
            "cannot read libfaust export baseline {}: {error}; create it with `cargo run -p xtask -- libfaust-export-check --bless`",
            workspace_relative_path(path)
        )
    })?;
    let symbols = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect::<BTreeSet<String>>();
    if symbols.is_empty() {
        return Err(format!(
            "libfaust export baseline is empty: {}",
            workspace_relative_path(path)
        )
        .into());
    }
    Ok(symbols)
}

fn write_export_baseline(
    path: &Path,
    symbols: &BTreeSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut output = String::from(
        "# Unified libfaust-rs exported C symbol baseline.\n\
         # Refresh intentionally with:\n\
         # cargo run -p xtask -- libfaust-export-check --bless\n",
    );
    for symbol in symbols {
        writeln!(output, "{symbol}")?;
    }
    fs::write(path, output)?;
    Ok(())
}

/// Builds and publishes the C/C++ distribution artifacts.
///
/// Rust library target names cannot contain hyphens, so `faust-ffi` builds
/// internal `faust_rs` artifacts and this packaging step publishes the stable
/// native names: `libfaust-rs.a` plus the platform dynamic-library equivalent.
/// Returns the published dynamic-library path.
pub(crate) fn build_libfaust_distribution(
    release: bool,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut command = Command::new("cargo");
    command.args(["build", "-p", "faust-ffi"]);
    if release {
        command.arg("--release");
    }
    let status = command.status()?;
    if !status.success() {
        return Err("cargo build -p faust-ffi failed".into());
    }

    let profile = if release { "release" } else { "debug" };
    let artifact_dir = workspace_root().join("target").join(profile);
    Ok(publish_libfaust_native_artifacts(&artifact_dir)?)
}

/// Parses and runs the explicit native C/C++ distribution workflow.
pub(crate) fn build_libfaust_distribution_command(
    args: BuildLibfaustArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let dynamic_library = build_libfaust_distribution(args.release)?;
    println!(
        "libfaust-rs native distribution ready: {}",
        workspace_relative_path(&dynamic_library)
    );
    Ok(())
}

pub(crate) fn publish_libfaust_native_artifacts(artifact_dir: &Path) -> Result<PathBuf, io::Error> {
    let static_source = artifact_dir.join(native_static_library_name("faust_rs"));
    let static_destination = artifact_dir.join(native_static_library_name("faust-rs"));
    publish_native_artifact(&static_source, &static_destination)?;

    let dynamic_source = artifact_dir.join(native_dynamic_library_name("faust_rs"));
    let dynamic_destination = artifact_dir.join(native_dynamic_library_name("faust-rs"));
    publish_native_artifact(&dynamic_source, &dynamic_destination)?;
    Ok(dynamic_destination)
}

fn publish_native_artifact(source: &Path, destination: &Path) -> Result<(), io::Error> {
    if source.is_file() {
        if destination.exists() {
            fs::remove_file(destination)?;
        }
        fs::rename(source, destination)?;
        return Ok(());
    }
    if destination.is_file() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("native library artifact not found at {}", source.display()),
    ))
}

pub(crate) fn native_static_library_name(stem: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{stem}.lib")
    } else {
        format!("lib{stem}.a")
    }
}

pub(crate) fn native_dynamic_library_name(stem: &str) -> String {
    format!(
        "{}{}{}",
        std::env::consts::DLL_PREFIX,
        stem,
        std::env::consts::DLL_SUFFIX
    )
}

fn expected_header_symbols(
    workspace: &Path,
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let headers = [
        workspace.join("crates/box-ffi/include/libfaust-box-c.h"),
        workspace.join("crates/signal-ffi/include/libfaust-signal-c.h"),
        workspace.join("crates/interp-ffi/include/interpreter-dsp-c.h"),
        workspace.join("crates/cranelift-ffi/include/cranelift-dsp-c.h"),
    ];
    let mut symbols = BTreeSet::new();
    for header in headers {
        for symbol in parse_c_header_function_symbols(&fs::read_to_string(&header)?) {
            symbols.insert(symbol);
        }
    }
    Ok(symbols)
}

fn parse_c_header_function_symbols(header: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    let mut pending = String::new();
    let header = strip_c_comments(header);

    for raw_line in header.lines() {
        let line = raw_line.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with("//")
            || line == "{"
            || line == "}"
            || line == "};"
            || line == "extern \"C\" {"
        {
            continue;
        }

        if pending.is_empty()
            && (line.starts_with("typedef ")
                || line.starts_with("enum ")
                || line.starts_with("struct "))
        {
            continue;
        }

        if pending.is_empty() && !line.contains('(') {
            continue;
        }

        if !pending.is_empty() {
            pending.push(' ');
        }
        pending.push_str(line);

        if pending.ends_with(';') {
            if let Some(name) = extract_c_function_name(&pending) {
                symbols.push(name);
            }
            pending.clear();
        }
    }

    symbols.sort();
    symbols.dedup();
    symbols
}

fn strip_c_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_block_comment = false;

    while let Some(character) = chars.next() {
        if in_block_comment {
            if character == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block_comment = false;
            } else if character == '\n' {
                output.push('\n');
            }
        } else if character == '/' && chars.peek() == Some(&'*') {
            chars.next();
            in_block_comment = true;
        } else if character == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for comment_character in chars.by_ref() {
                if comment_character == '\n' {
                    output.push('\n');
                    break;
                }
            }
        } else {
            output.push(character);
        }
    }

    output
}

fn exported_dynamic_symbols(path: &Path) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let output = if cfg!(target_os = "macos") {
        Command::new("nm").args(["-gU"]).arg(path).output()?
    } else if cfg!(target_os = "windows") {
        Command::new("dumpbin").arg("/exports").arg(path).output()?
    } else {
        Command::new("nm")
            .args(["-D", "--defined-only"])
            .arg(path)
            .output()?
    };

    if !output.status.success() {
        return Err(format!(
            "failed to inspect dynamic symbols for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_exported_symbol_lines(&stdout))
}

fn parse_exported_symbol_lines(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| {
            line.split_whitespace()
                .last()
                .map(|name| name.trim_start_matches('_').to_string())
                .filter(|name| is_libfaust_c_symbol(name))
        })
        .collect()
}

fn is_libfaust_c_symbol(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "C",
        "buildUserInterface",
        "clear",
        "clone",
        "compute",
        "create",
        "delete",
        "destroy",
        "expand",
        "free",
        "generate",
        "get",
        "init",
        "instance",
        "metadata",
        "read",
        "register",
        "start",
        "stop",
        "unregister",
        "write",
    ];

    !name.is_empty()
        && name
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
        && PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

fn syntax_check_headers(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = workspace.join("target/libfaust-export-check");
    fs::create_dir_all(&out_dir)?;

    let c_file = out_dir.join("smoke-core.c");
    fs::write(
        &c_file,
        r#"#include "libfaust-box-c.h"
#include "libfaust-signal-c.h"

int main(void) {
    Signal s = CsigInput(0);
    Box b = CboxWire();
    return (s == 0 || b == 0) ? 0 : 0;
}
"#,
    )?;

    let interpreter_c_file = out_dir.join("smoke-interpreter.c");
    fs::write(
        &interpreter_c_file,
        r#"#include <stddef.h>
#include "interpreter-dsp-c.h"

int main(void) {
    _Static_assert(sizeof(FAUSTFLOAT) == sizeof(float), "FAUSTFLOAT must default to float");
    _Static_assert(sizeof(UIGlue) == 14 * sizeof(void*), "UIGlue size");
    _Static_assert(_Alignof(UIGlue) == _Alignof(void*), "UIGlue alignment");
    _Static_assert(offsetof(UIGlue, ui_interface) == 0 * sizeof(void*), "ui_interface offset");
    _Static_assert(offsetof(UIGlue, open_tab_box) == 1 * sizeof(void*), "open_tab_box offset");
    _Static_assert(offsetof(UIGlue, open_horizontal_box) == 2 * sizeof(void*), "open_horizontal_box offset");
    _Static_assert(offsetof(UIGlue, open_vertical_box) == 3 * sizeof(void*), "open_vertical_box offset");
    _Static_assert(offsetof(UIGlue, close_box) == 4 * sizeof(void*), "close_box offset");
    _Static_assert(offsetof(UIGlue, add_button) == 5 * sizeof(void*), "add_button offset");
    _Static_assert(offsetof(UIGlue, add_check_button) == 6 * sizeof(void*), "add_check_button offset");
    _Static_assert(offsetof(UIGlue, add_vertical_slider) == 7 * sizeof(void*), "add_vertical_slider offset");
    _Static_assert(offsetof(UIGlue, add_horizontal_slider) == 8 * sizeof(void*), "add_horizontal_slider offset");
    _Static_assert(offsetof(UIGlue, add_num_entry) == 9 * sizeof(void*), "add_num_entry offset");
    _Static_assert(offsetof(UIGlue, add_horizontal_bargraph) == 10 * sizeof(void*), "add_horizontal_bargraph offset");
    _Static_assert(offsetof(UIGlue, add_vertical_bargraph) == 11 * sizeof(void*), "add_vertical_bargraph offset");
    _Static_assert(offsetof(UIGlue, add_soundfile) == 12 * sizeof(void*), "add_soundfile offset");
    _Static_assert(offsetof(UIGlue, declare) == 13 * sizeof(void*), "declare offset");
    _Static_assert(sizeof(MetaGlue) == 2 * sizeof(void*), "MetaGlue size");
    _Static_assert(_Alignof(MetaGlue) == _Alignof(void*), "MetaGlue alignment");
    _Static_assert(offsetof(MetaGlue, meta_interface) == 0, "meta_interface offset");
    _Static_assert(offsetof(MetaGlue, declare) == sizeof(void*), "meta declare offset");
    UIGlue ui = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0};
    MetaGlue meta = {0, 0};
    interpreter_dsp_factory* factory = getCInterpreterDSPFactoryFromSHAKey("missing");
    return (factory == 0 || ui.ui_interface == 0 || meta.meta_interface == 0) ? 0 : 0;
}
"#,
    )?;

    let cranelift_c_file = out_dir.join("smoke-cranelift.c");
    fs::write(
        &cranelift_c_file,
        r#"#include <stddef.h>
#include "cranelift-dsp-c.h"

int main(void) {
    _Static_assert(sizeof(FAUSTFLOAT) == sizeof(float), "FAUSTFLOAT must default to float");
    _Static_assert(sizeof(UIGlue) == 14 * sizeof(void*), "UIGlue size");
    _Static_assert(_Alignof(UIGlue) == _Alignof(void*), "UIGlue alignment");
    _Static_assert(offsetof(UIGlue, ui_interface) == 0 * sizeof(void*), "ui_interface offset");
    _Static_assert(offsetof(UIGlue, open_tab_box) == 1 * sizeof(void*), "open_tab_box offset");
    _Static_assert(offsetof(UIGlue, open_horizontal_box) == 2 * sizeof(void*), "open_horizontal_box offset");
    _Static_assert(offsetof(UIGlue, open_vertical_box) == 3 * sizeof(void*), "open_vertical_box offset");
    _Static_assert(offsetof(UIGlue, close_box) == 4 * sizeof(void*), "close_box offset");
    _Static_assert(offsetof(UIGlue, add_button) == 5 * sizeof(void*), "add_button offset");
    _Static_assert(offsetof(UIGlue, add_check_button) == 6 * sizeof(void*), "add_check_button offset");
    _Static_assert(offsetof(UIGlue, add_vertical_slider) == 7 * sizeof(void*), "add_vertical_slider offset");
    _Static_assert(offsetof(UIGlue, add_horizontal_slider) == 8 * sizeof(void*), "add_horizontal_slider offset");
    _Static_assert(offsetof(UIGlue, add_num_entry) == 9 * sizeof(void*), "add_num_entry offset");
    _Static_assert(offsetof(UIGlue, add_horizontal_bargraph) == 10 * sizeof(void*), "add_horizontal_bargraph offset");
    _Static_assert(offsetof(UIGlue, add_vertical_bargraph) == 11 * sizeof(void*), "add_vertical_bargraph offset");
    _Static_assert(offsetof(UIGlue, add_soundfile) == 12 * sizeof(void*), "add_soundfile offset");
    _Static_assert(offsetof(UIGlue, declare) == 13 * sizeof(void*), "declare offset");
    _Static_assert(sizeof(MetaGlue) == 2 * sizeof(void*), "MetaGlue size");
    _Static_assert(_Alignof(MetaGlue) == _Alignof(void*), "MetaGlue alignment");
    _Static_assert(offsetof(MetaGlue, meta_interface) == 0, "meta_interface offset");
    _Static_assert(offsetof(MetaGlue, declare) == sizeof(void*), "meta declare offset");
    UIGlue ui = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0};
    MetaGlue meta = {0, 0};
    cranelift_dsp_factory* factory = getCCraneliftDSPFactoryFromSHAKey("missing");
    return (factory == 0 || ui.ui_interface == 0 || meta.meta_interface == 0) ? 0 : 0;
}
"#,
    )?;

    let interpreter_cpp_file = out_dir.join("smoke-interpreter.cpp");
    fs::write(
        &interpreter_cpp_file,
        r#"#include <cstddef>
#include "interpreter-dsp-c.h"

static_assert(sizeof(FAUSTFLOAT) == sizeof(float));
static_assert(sizeof(UIGlue) == 14 * sizeof(void*));
static_assert(alignof(UIGlue) == alignof(void*));
static_assert(offsetof(UIGlue, declare) == 13 * sizeof(void*));
static_assert(sizeof(MetaGlue) == 2 * sizeof(void*));
static_assert(alignof(MetaGlue) == alignof(void*));

int main() {
    UIGlue ui{nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
              nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr};
    MetaGlue meta{nullptr, nullptr};
    return (ui.ui_interface == nullptr || meta.meta_interface == nullptr) ? 0 : 0;
}
"#,
    )?;

    let cranelift_cpp_file = out_dir.join("smoke-cranelift.cpp");
    fs::write(
        &cranelift_cpp_file,
        r#"#include <cstddef>
#include "cranelift-dsp-c.h"

static_assert(sizeof(FAUSTFLOAT) == sizeof(float));
static_assert(sizeof(UIGlue) == 14 * sizeof(void*));
static_assert(alignof(UIGlue) == alignof(void*));
static_assert(offsetof(UIGlue, declare) == 13 * sizeof(void*));
static_assert(sizeof(MetaGlue) == 2 * sizeof(void*));
static_assert(alignof(MetaGlue) == alignof(void*));

int main() {
    UIGlue ui{nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
              nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr};
    MetaGlue meta{nullptr, nullptr};
    return (ui.ui_interface == nullptr || meta.meta_interface == nullptr) ? 0 : 0;
}
"#,
    )?;

    let cpp_file = out_dir.join("smoke.cpp");
    fs::write(
        &cpp_file,
        r#"#include "libfaust-box.h"
#include "libfaust-signal.h"

int main() {
    Signal x = sigInput(0);
    Signal y = sigMul(x, sigReal(0.5));
    int op = 0;
    Signal a = nullptr;
    Signal b = nullptr;
    return isSigBinOp(y, op, a, b) ? 0 : 0;
}
"#,
    )?;

    syntax_check_c_like(&c_file, "c")?;
    syntax_check_c_like(&interpreter_c_file, "c")?;
    syntax_check_c_like(&cranelift_c_file, "c")?;
    syntax_check_c_like(&cpp_file, "c++")?;
    syntax_check_c_like(&interpreter_cpp_file, "c++")?;
    syntax_check_c_like(&cranelift_cpp_file, "c++")?;
    Ok(())
}

fn syntax_check_c_like(path: &Path, language: &str) -> Result<(), Box<dyn std::error::Error>> {
    let workspace = workspace_root();
    let include_dirs = [
        workspace.join("crates/box-ffi/include"),
        workspace.join("crates/signal-ffi/include"),
        workspace.join("crates/interp-ffi/include"),
        workspace.join("crates/cranelift-ffi/include"),
    ];

    let compiler_var = if language == "c" { "CC" } else { "CXX" };
    let default_compiler = if cfg!(target_os = "windows") {
        "cl"
    } else if language == "c" {
        "cc"
    } else {
        "c++"
    };
    let compiler = std::env::var(compiler_var).unwrap_or_else(|_| default_compiler.to_string());

    let mut command = Command::new(&compiler);
    if cfg!(target_os = "windows") && compiler.ends_with("cl") {
        command.arg("/nologo").arg("/Zs");
        command.arg(if language == "c" {
            "/std:c11"
        } else {
            "/std:c++17"
        });
        for include_dir in include_dirs {
            command.arg(format!("/I{}", include_dir.display()));
        }
        command.arg(path);
    } else {
        command.arg(if language == "c" {
            "-std=c11"
        } else {
            "-std=c++17"
        });
        command.arg("-fsyntax-only");
        for include_dir in include_dirs {
            command.arg("-I").arg(include_dir);
        }
        command.arg(path);
    }

    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "{} syntax check failed for {}:\n{}{}",
            compiler,
            path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_header_function_symbols_without_libfaust_macro() {
        let header = r#"
            #ifdef __cplusplus
            extern "C" {
            #endif
            /* Function-like text in comments must not be parsed:
             * fakeFunction(the, words);
             */
            typedef CTree* Signal;
            typedef void (*callbackFn)(void* context);
            enum SType { kSInt, kSReal };
            void createLibContext(void);
            Signal CsigFFun(enum SType rtype, const char** names,
                            enum SType* atypes, const char* incfile);
            #ifdef __cplusplus
            }
            #endif
        "#;

        assert_eq!(
            parse_c_header_function_symbols(header),
            vec!["CsigFFun".to_string(), "createLibContext".to_string()]
        );
    }

    #[test]
    fn strips_line_and_block_comments_without_joining_declarations() {
        let header = r#"
            /* fakeBlock(one); */
            void realOne(void); // fakeLine(two);
            /*
             * fakeMultiline(
             *     three);
             */
            void realTwo(void);
        "#;

        assert_eq!(
            parse_c_header_function_symbols(header),
            vec!["realOne".to_string(), "realTwo".to_string()]
        );
    }

    #[test]
    fn parses_nm_and_dumpbin_symbol_lines() {
        let output = r#"
            0000000000012340 T _CsigInt
            0000000000012350 T _createLibContext
            0000000000012358 T _getCInterpreterDSPFactoryFromSHAKey
              12    B 0000000180001230 CboxInt
            0000000000012360 T _rust_internal_helper
            ordinal hint RVA      name
        "#;

        let symbols = parse_exported_symbol_lines(output);

        assert!(symbols.contains("CsigInt"));
        assert!(symbols.contains("CboxInt"));
        assert!(symbols.contains("createLibContext"));
        assert!(symbols.contains("getCInterpreterDSPFactoryFromSHAKey"));
        assert!(!symbols.contains("rust_internal_helper"));
        assert!(!symbols.contains("name"));
    }
}
