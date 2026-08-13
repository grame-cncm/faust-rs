//! Focused `-mem0` structural differential against Faust C++ `8eebea429`.
//!
//! The comparison is semantic because faust-rs intentionally fixes the
//! reference object-size/count sentinel and lifecycle/clone defects. The test
//! skips when the pinned compiler is unavailable; `FAUST_CPP_BIN` can select
//! an equivalent build.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use codegen::backends::cpp::CppOptions;
use codegen::memory_layout::{MemoryLayoutFlavor, MemoryManagerMode};
use compiler::{Compiler, SignalFirLane};
use serde_json::Value;

const PINNED_FAUST: &str = "/Users/letz/Developpements/RUST/faust/build/bin/faust";

#[derive(Debug, PartialEq, Eq)]
struct LegacyZone {
    name: String,
    memory_type: String,
    elements: String,
    size_bytes: String,
    reads: u64,
    writes: u64,
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/impulse-tests/dsp-mem0")
        .join(name)
}

fn reference_binary() -> Option<PathBuf> {
    let path = std::env::var_os("FAUST_CPP_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(PINNED_FAUST));
    path.is_file().then_some(path)
}

fn reference_cpp(faust: &Path, dsp: &Path) -> String {
    let output = Command::new(faust)
        .args(["-lang", "cpp", "-mem0"])
        .arg(dsp)
        .output()
        .expect("run pinned Faust C++");
    assert!(
        output.status.success(),
        "pinned Faust C++ rejected {}:\n{}",
        dsp.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("reference C++ is UTF-8")
}

fn reference_json(faust: &Path, dsp: &Path) -> Value {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after Unix epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "faust-rs-mem0-differential-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).expect("create differential scratch directory");
    let source = directory.join("probe.dsp");
    std::fs::copy(dsp, &source).expect("copy differential DSP");
    let output = Command::new(faust)
        .current_dir(&directory)
        .args([
            "-lang",
            "cpp",
            "-mem0",
            "-json",
            "probe.dsp",
            "-o",
            "probe.cpp",
        ])
        .output()
        .expect("run pinned Faust C++ JSON generator");
    let json_path = directory.join("probe.dsp.json");
    let json = std::fs::read_to_string(&json_path);
    std::fs::remove_dir_all(&directory).expect("remove differential scratch directory");
    assert!(
        output.status.success(),
        "pinned Faust C++ rejected {}:\n{}",
        dsp.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&json.expect("pinned Faust C++ JSON companion"))
        .expect("pinned Faust C++ emitted valid JSON")
}

fn rust_json(dsp: &Path) -> Value {
    let json = Compiler::new()
        .compile_file_to_json_with_compile_options_and_memory(
            dsp,
            &[],
            SignalFirLane::TransformFastLane,
            "-lang cpp -mem0 -single".to_owned(),
            Some(MemoryLayoutFlavor::Cpp),
        )
        .unwrap_or_else(|error| panic!("faust-rs rejected {}: {error}", dsp.display()));
    serde_json::from_str(&json).expect("faust-rs emitted valid JSON")
}

fn rust_cpp(dsp: &Path) -> String {
    Compiler::new()
        .compile_file_to_cpp(
            dsp,
            &[],
            &CppOptions {
                memory_manager_mode: MemoryManagerMode::Mem0,
                ..CppOptions::default()
            },
        )
        .unwrap_or_else(|error| panic!("faust-rs rejected {}: {error}", dsp.display()))
}

fn parse_zones(source: &str) -> Vec<LegacyZone> {
    source
        .lines()
        .filter_map(|line| {
            let (_, arguments) = line.split_once("->info(")?;
            let arguments = arguments.strip_suffix(");")?;
            let fields: Vec<_> = arguments.split(',').map(str::trim).collect();
            assert_eq!(fields.len(), 6, "unexpected memory-info call: {line}");
            Some(LegacyZone {
                name: fields[0].trim_matches('"').to_owned(),
                memory_type: fields[1]
                    .strip_prefix("dsp_memory_manager::")
                    .unwrap_or(fields[1])
                    .to_owned(),
                elements: fields[2].to_owned(),
                size_bytes: fields[3].to_owned(),
                reads: fields[4].parse().expect("numeric read count"),
                writes: fields[5].parse().expect("numeric write count"),
            })
        })
        .collect()
}

fn begin_count(source: &str) -> usize {
    source
        .lines()
        .find_map(|line| {
            let (_, tail) = line.split_once("->begin(")?;
            tail.strip_suffix(");")?.parse().ok()
        })
        .expect("memory manager begin count")
}

#[test]
fn delay_layout_preserves_every_unaffected_legacy_field() {
    let Some(faust) = reference_binary() else {
        eprintln!("skipping mem0 C++ differential: pinned Faust binary unavailable");
        return;
    };
    let dsp = fixture("mem0_delays.dsp");
    let reference = reference_cpp(&faust, &dsp);
    let rust = rust_cpp(&dsp);
    let reference_zones = parse_zones(&reference);
    let rust_zones = parse_zones(&rust);

    assert_eq!(begin_count(&reference), reference_zones.len());
    assert_eq!(begin_count(&rust), rust_zones.len());
    assert_eq!(reference_zones.len(), 2);
    assert_eq!(rust_zones.len(), 2);

    let reference_object = &reference_zones[0];
    let rust_object = &rust_zones[0];
    assert_eq!(reference_object.memory_type, "kObj_ptr");
    assert_eq!(rust_object.memory_type, "kObj_ptr");
    assert_eq!(reference_object.reads, rust_object.reads);
    assert_eq!(reference_object.writes, rust_object.writes);
    assert_eq!(reference_object.elements, "0");
    assert_eq!(rust_object.elements, "1");
    assert!(rust_object.size_bytes.starts_with("sizeof("));

    let reference_delay = &reference_zones[1];
    let rust_delay = &rust_zones[1];
    assert_eq!(reference_delay.memory_type, rust_delay.memory_type);
    assert_eq!(reference_delay.elements, rust_delay.elements);
    assert_eq!(reference_delay.size_bytes, rust_delay.size_bytes);
    assert_eq!(reference_delay.reads, rust_delay.reads);
    assert_eq!(reference_delay.writes, rust_delay.writes);

    assert!(reference.contains("virtual void init(int sample_rate) {}"));
    assert!(reference.contains("TODO: deep copy would be needed here"));
    assert!(rust.contains("classInit(sample_rate);\n        instanceInit(sample_rate);"));
    assert!(rust.contains("std::memcpy(copy->"));
    assert!(rust.contains("dsp_memory_manager* owner = typed->fOwnerManager"));
}

#[test]
fn table_element_count_fix_is_the_only_legacy_table_field_exception() {
    let Some(faust) = reference_binary() else {
        eprintln!("skipping mem0 C++ differential: pinned Faust binary unavailable");
        return;
    };
    let dsp = fixture("mem0_tables.dsp");
    let reference_zones = parse_zones(&reference_cpp(&faust, &dsp));
    let rust_zones = parse_zones(&rust_cpp(&dsp));
    assert_eq!(reference_zones.len(), rust_zones.len());

    for memory_type in ["kInt32_ptr", "kFloat_ptr"] {
        let reference = reference_zones
            .iter()
            .find(|zone| zone.memory_type == memory_type)
            .expect("reference table zone");
        let rust = rust_zones
            .iter()
            .find(|zone| zone.memory_type == memory_type)
            .expect("Rust table zone");
        assert_eq!(reference.size_bytes, rust.size_bytes);
        assert_eq!(reference.reads, rust.reads);
        assert_eq!(reference.writes, rust.writes);
        assert_eq!(reference.elements, "0", "pinned sentinel baseline");
        assert_ne!(rust.elements, "0", "faust-rs reports the real table count");
    }
}

#[test]
fn delay_compute_cost_matches_pinned_cpp_exactly() {
    let Some(faust) = reference_binary() else {
        eprintln!("skipping mem0 C++ differential: pinned Faust binary unavailable");
        return;
    };
    let dsp = fixture("mem0_delays.dsp");
    let reference = reference_json(&faust, &dsp);
    let rust = rust_json(&dsp);

    assert_eq!(rust["compute_cost"], reference["compute_cost"]);
    assert_eq!(rust["compute_cost"][0]["load"], 13);
    assert_eq!(rust["compute_cost"][0]["store"], 4);
    assert_eq!(rust["compute_cost"][0]["declare"], 2);
    assert_eq!(rust["compute_cost"][0]["number"], 8);
}

/// The JSON `memory_layout` must name the very identifiers the `-mem0` C++ it
/// describes actually emits — which both compilers derive from the class name
/// (default `"mydsp"`), never from the source stem. A mismatch means the JSON
/// companion documents zones that do not exist in the generated `.cpp`.
#[test]
fn json_memory_layout_names_the_identifiers_the_generated_cpp_emits() {
    let Some(faust) = reference_binary() else {
        eprintln!("skipping mem0 C++ differential: pinned Faust binary unavailable");
        return;
    };
    let dsp = fixture("mem0_delays.dsp");
    let zone_names = |source: &str| -> Vec<String> {
        parse_zones(source)
            .into_iter()
            .map(|zone| zone.name)
            .collect()
    };
    let cpp_names = zone_names(&rust_cpp(&dsp));
    let json_names = rust_json(&dsp)["memory_layout"]
        .as_array()
        .expect("memory_layout array")
        .iter()
        .map(|zone| zone["name"].as_str().expect("zone name").to_owned())
        .collect::<Vec<_>>();

    // The pinned reference anchors the convention this fixture must follow.
    assert_eq!(zone_names(&reference_cpp(&faust, &dsp))[0], "mydsp");

    // Every zone the C++ registers with the manager must appear in the JSON
    // under the same name. The JSON additionally describes non-allocated
    // scalars, so this is containment rather than equality.
    assert_eq!(cpp_names[0], "mydsp");
    for name in &cpp_names {
        assert!(
            json_names.contains(name),
            "JSON memory_layout {json_names:?} is missing C++ zone {name:?}"
        );
    }
    assert!(
        json_names
            .iter()
            .all(|name| !name.starts_with("mem0_delays")),
        "zone named after the source stem: {json_names:?}"
    );
}
