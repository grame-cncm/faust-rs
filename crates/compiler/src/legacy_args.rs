//! The C++ Faust single-dash option spellings, rewritten to the long flags of
//! the Rust command lines (see [`normalize_legacy_args`]).

/// Normalizes legacy Faust-style flags to the current `clap` surface.
///
/// The one table of the C++ single-dash spellings (`-pn NAME` →
/// `--process-name NAME`, `-vec` → `--vec`, `-ss N` →
/// `--scheduling-strategy N`, ...): the `faust-rs` binary and `faustprobe`,
/// whose compiler options carry the same long names, both run their
/// command line through it before Clap, which cannot parse a multi-letter
/// short flag. A flag that takes a value keeps the next argument as it is.
pub fn normalize_legacy_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "-lang" {
            normalized.push("--lang".to_owned());
            if let Some(value) = it.next() {
                let mapped = match value.as_str() {
                    "-c" => "c".to_owned(),
                    "-cpp" => "cpp".to_owned(),
                    "-fir" => "fir".to_owned(),
                    "-interp" => "interp".to_owned(),
                    _ => value,
                };
                normalized.push(mapped);
            }
            continue;
        }
        if arg == "-pn" {
            normalized.push("--process-name".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-cn" {
            normalized.push("--class-name".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-scn" {
            normalized.push("--super-class-name".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-double" {
            normalized.push("--double".to_owned());
            continue;
        }
        if arg == "-json" {
            normalized.push("--json".to_owned());
            continue;
        }
        if arg == "-mem" || arg == "-mem0" {
            normalized.push("--memory-manager".to_owned());
            continue;
        }
        if arg == "-version" {
            normalized.push("--version".to_owned());
            continue;
        }
        if arg == "-libdir" {
            normalized.push("--libdir".to_owned());
            continue;
        }
        if arg == "-includedir" {
            normalized.push("--includedir".to_owned());
            continue;
        }
        if arg == "-archdir" {
            normalized.push("--archdir".to_owned());
            continue;
        }
        if arg == "-dspdir" {
            normalized.push("--dspdir".to_owned());
            continue;
        }
        if arg == "-pathslist" {
            normalized.push("--pathslist".to_owned());
            continue;
        }
        if arg == "-mcd" {
            normalized.push("--mcd".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-bra-tape" {
            normalized.push("--bra-tape".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-ct" {
            normalized.push("--check-table".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-table-init" {
            normalized.push("--table-init".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-dlt" {
            normalized.push("--dlt".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-vec" {
            normalized.push("--vec".to_owned());
            continue;
        }
        if arg == "-vs" {
            normalized.push("--vs".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-lv" {
            normalized.push("--lv".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-ss" {
            normalized.push("--scheduling-strategy".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-ec" {
            normalized.push("--ec".to_owned());
            continue;
        }
        if arg == "-os" {
            normalized.push("--os".to_owned());
            continue;
        }
        if arg == "-time" {
            normalized.push("--compilation-time".to_owned());
            continue;
        }
        if arg == "-svg" {
            normalized.push("--svg".to_owned());
            continue;
        }
        if arg == "-blur" {
            normalized.push("--shadow-blur".to_owned());
            continue;
        }
        if arg == "-sc" {
            normalized.push("--scaled-svg".to_owned());
            continue;
        }
        if arg == "-drf" {
            normalized.push("--draw-route-frame".to_owned());
            continue;
        }
        if arg == "-mns" {
            normalized.push("--max-name-size".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-f" {
            normalized.push("--fold".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-fc" {
            normalized.push("--fold-complexity".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-timeout" {
            normalized.push("--timeout".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        normalized.push(arg);
    }
    normalized
}
