//! Cmajor backend integration tests.
//!
//! These tests deliberately compile self-contained Faust definitions through
//! the production compiler facade and signal-to-FIR lane. They
//! do not depend on an installed Faust compiler, Faust libraries, or Cmajor
//! SDK. Optional Cmajor frontend/runtime validation is layered separately.
//! Set `CMAJ_BIN` to enable Cmajor syntax and generated-C++ runtime checks,
//! `FAUST_CPP_BIN` to enable the pinned C++ Faust differential, and optionally
//! `CMAJ_CXX` to select the C++ compiler used by the runtime harness.
//!
//! Source provenance and acceptance contract:
//! `porting/cmajor-backend-port-and-test-plan-2026-08-04-en.md` C1-C6.

use codegen::backends::cmajor::{
    CmajorOptions, CmajorRealType, CodegenErrorCode, generate_cmajor_module,
};
use compiler::{Compiler, ControlRateMode, ProcessingApi, RealType, SignalFirLane};
use std::process::Command;

/// Compiles a Faust source with the execution shape Cmajor intrinsically uses.
fn cmajor(source_name: &str, source: &str) -> String {
    cmajor_with(source_name, source, &CmajorOptions::default())
}

/// Compiles with precision synchronized across lowering and source emission.
fn cmajor_with(source_name: &str, source: &str, options: &CmajorOptions) -> String {
    let real_type = match options.real_type {
        CmajorRealType::Float32 => RealType::Float32,
        CmajorRealType::Float64 => RealType::Float64,
    };
    let compiler = Compiler::new()
        .with_real_type(real_type)
        .with_control_rate_mode(ControlRateMode::External)
        .with_processing_api(ProcessingApi::OneSample);
    compiler
        .compile_source_to_cmajor_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
        .expect("Cmajor facade emission must succeed")
}

/// Runs the optional external Cmajor syntax gate when `CMAJ_BIN` is set.
fn assert_cmajor_frontend(source_name: &str, source: &str) {
    let Ok(cmaj) = std::env::var("CMAJ_BIN") else {
        return;
    };
    let path = std::env::temp_dir().join(format!(
        "faust-rs-cmajor-{source_name}-{}.cmajor",
        std::process::id()
    ));
    std::fs::write(&path, source).expect("write temporary Cmajor source");
    let result = Command::new(cmaj)
        .args(["generate", "--target=syntaxtree"])
        .arg(&path)
        .output()
        .expect("run Cmajor frontend");
    std::fs::remove_file(&path).expect("remove temporary Cmajor source");
    assert!(
        result.status.success(),
        "Cmajor rejected {source_name}:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

/// Generates the same self-contained source with the opt-in pinned C++ Faust.
fn cpp_cmajor(source_name: &str, source: &str, double: bool) -> Option<String> {
    let faust = std::env::var("FAUST_CPP_BIN").ok()?;
    let path = std::env::temp_dir().join(format!(
        "faust-rs-cmajor-cpp-{source_name}-{}.dsp",
        std::process::id()
    ));
    std::fs::write(&path, source).expect("write temporary C++ differential DSP");
    let mut command = Command::new(faust);
    command.args(["-lang", "cmajor", "-cn", "mydsp"]);
    if double {
        command.arg("-double");
    }
    let result = command
        .arg(&path)
        .output()
        .expect("run pinned C++ Faust Cmajor backend");
    std::fs::remove_file(&path).expect("remove temporary C++ differential DSP");
    assert!(
        result.status.success(),
        "C++ Faust rejected {source_name}:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    Some(String::from_utf8(result.stdout).expect("C++ Cmajor source is UTF-8"))
}

/// Narrow observable contract used for C++/Rust differential comparison.
///
/// Formatting, helper layout, and the intentionally adapted lifecycle are
/// excluded. Streams, public events, handlers, tick advancement, and bargraph
/// scheduling are retained. Table declarations and `.at` calls are omitted:
/// the pinned C++ optimizer constant-folds some tables that canonical Rust FIR
/// deliberately retains, without changing the public or runtime contract.
#[derive(Debug, PartialEq, Eq)]
struct CmajorContract {
    streams: Vec<String>,
    events: Vec<String>,
    handlers: Vec<String>,
    advance_count: usize,
    bargraph_clock: bool,
}

fn cmajor_contract(source: &str) -> CmajorContract {
    let mut streams = Vec::new();
    let mut events = Vec::new();
    let mut handlers = Vec::new();
    for line in source.lines().map(str::trim) {
        let compact = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if compact.starts_with("input stream ") || compact.starts_with("output stream ") {
            streams.push(compact.trim_end_matches(';').to_owned());
        } else if compact.starts_with("input event ") || compact.starts_with("output event ") {
            let declaration = compact.split("[[").next().unwrap_or(&compact).trim();
            events.push(declaration.trim_end_matches(';').trim().to_owned());
        } else if compact.starts_with("event event") {
            let signature = compact.split('{').next().unwrap_or(&compact);
            handlers.push(signature.chars().filter(|ch| !ch.is_whitespace()).collect());
        }
    }
    let no_space: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();
    CmajorContract {
        streams,
        events,
        handlers,
        advance_count: no_space.matches("advance();").count(),
        bargraph_clock: no_space.contains("fControlSlice--==0"),
    }
}

/// Generates C++ through Cmajor, compiles a tiny host, and returns 16 samples.
fn run_cmajor_generated_cpp(
    cmaj: &str,
    directory: &std::path::Path,
    optimization: &str,
) -> Vec<f32> {
    let patch = directory.join("runtime.cmajorpatch");
    let header = directory.join(format!("runtime-{optimization}.hpp"));
    let result = Command::new(cmaj)
        .arg("generate")
        .arg(optimization)
        .arg("--target=cpp")
        .arg(format!("--output={}", header.display()))
        .arg(&patch)
        .output()
        .expect("run Cmajor C++ generator");
    assert!(
        result.status.success(),
        "Cmajor C++ generation {optimization} failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    let header_name = header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("UTF-8 Cmajor header name");
    let harness = directory.join(format!("harness-{optimization}.cpp"));
    std::fs::write(
        &harness,
        format!(
            "#include <cstdio>\n#include \"{header_name}\"\n\
             int main() {{ RuntimeMain dsp; dsp.initialise(1, 48000.0); \
             dsp.advance(16); float samples[16] = {{}}; \
             dsp.copyOutputFrames(RuntimeMain::getEndpointHandleForName(\"audioOut\"), samples, 16); \
             for (float sample : samples) std::printf(\"%.9g\\n\", double(sample)); }}\n"
        ),
    )
    .expect("write Cmajor C++ runtime harness");
    let executable = directory.join(format!("runtime-{optimization}"));
    let cxx = std::env::var("CMAJ_CXX").unwrap_or_else(|_| "c++".to_owned());
    let build = Command::new(cxx)
        .args(["-std=c++17", "-O2"])
        .arg(&harness)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("compile Cmajor-generated C++ runtime harness");
    assert!(
        build.status.success(),
        "Cmajor C++ harness build failed:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&executable)
        .output()
        .expect("run Cmajor-generated C++ runtime harness");
    assert!(run.status.success(), "Cmajor C++ runtime harness failed");
    String::from_utf8(run.stdout)
        .expect("runtime samples are UTF-8")
        .lines()
        .map(|line| line.parse::<f32>().expect("runtime sample is numeric"))
        .collect()
}

#[test]
fn controls_emit_annotated_events_and_dirty_handlers() {
    let text = cmajor(
        "ui.dsp",
        "process = _ * hslider(\"gain[unit:dB]\", 0.5, 0, 1, 0.01) \
         * checkbox(\"on\") + button(\"trig\") \
         + nentry(\"num\", 2, 0, 10, 1);",
    );
    assert!(
        text.contains("input event float32 eventfHslider0"),
        "{text}"
    );
    assert!(text.contains("min: 0.0f, max: 1.0f"), "{text}");
    assert!(text.contains("meta_unit0: \"dB\""), "{text}");
    assert!(text.contains("event eventfHslider0(float32 val)"), "{text}");
    assert!(text.contains("fUpdated ||= (fHslider0 != val)"), "{text}");
    assert!(
        text.contains("latching, text: \"off|on\", boolean"),
        "{text}"
    );
    assert_cmajor_frontend("ui", &text);
}

#[test]
fn bargraphs_emit_rate_limited_output_events() {
    let text = cmajor(
        "bar.dsp",
        "process = _ <: attach(_, vbargraph(\"lvl\", 0, 1)), \
         hbargraph(\"pk\", 0, 2);",
    );
    assert!(
        text.contains("output event float32 eventfVbargraph0"),
        "{text}"
    );
    assert!(text.contains("int fControlSlice;"), "{text}");
    assert!(
        text.contains("if (fControlSlice == 0) { eventfVbargraph0 <- fVbargraph0; }"),
        "{text}"
    );
    assert!(
        text.contains("fControlSlice = int(processor.frequency) / 50;"),
        "{text}"
    );
    assert!(text.contains("if (fControlSlice-- == 0)"), "{text}");
    assert_cmajor_frontend("bargraph", &text);
}

#[test]
fn float64_precision_reaches_streams_controls_and_literals() {
    let options = CmajorOptions {
        real_type: CmajorRealType::Float64,
        ..CmajorOptions::default()
    };
    let text = cmajor_with(
        "double.dsp",
        "process = _ * hslider(\"gain\", 0.5, 0, 1, 0.01);",
        &options,
    );
    assert!(text.contains("input stream float64 input0;"), "{text}");
    assert!(
        text.contains("input event float64 eventfHslider0"),
        "{text}"
    );
    assert!(text.contains("init: 0.5, step: 0.01"), "{text}");
    assert!(!text.contains("0.5f"), "{text}");
    assert_cmajor_frontend("float64", &text);
}

#[test]
fn readonly_table_emits_concrete_cmajor_storage() {
    let text = cmajor("rdtable.dsp", "process = rdtable(8, 0.25, int(_));");
    assert!(text.contains("[8]"), "{text}");
    assert!(text.contains(".at(") || text.contains("[int("), "{text}");
    assert_cmajor_frontend("rdtable", &text);
}

#[test]
fn waveform_table_emits_initializer_and_indexed_read() {
    let text = cmajor(
        "waveform.dsp",
        "wave(x) = waveform { 10, 20, 30, 40 }, int(x) : rdtable; \
         process = wave;",
    );
    assert!(text.contains("[4]"), "{text}");
    assert!(text.contains("10.0f") || text.contains("10"), "{text}");
    assert_cmajor_frontend("waveform", &text);
}

#[test]
fn writable_table_emits_runtime_store_and_wrapped_access() {
    let text = cmajor(
        "rwtable.dsp",
        "process = rwtable(8, 0.0, int(_), _ * 0.5, int(_));",
    );
    assert!(text.contains("[8]"), "{text}");
    assert!(text.contains(".at("), "{text}");
    assert_cmajor_frontend("rwtable", &text);
}

#[test]
fn generated_table_specializes_fill_helper_to_concrete_size() {
    let source = "generator = +(1) ~ _; process = rdtable(8, generator, int(_));";
    let text = cmajor("generated-table.dsp", source);
    assert!(text.contains("[8]"), "{text}");
    assert_cmajor_frontend("generated-table", &text);

    let other = cmajor(
        "generated-table-16.dsp",
        "generator = +(1) ~ _; process = rdtable(16, generator, int(_));",
    );
    assert!(other.contains("[16]"), "{other}");
    assert_cmajor_frontend("generated-table-16", &other);

    let repeated = cmajor("generated-table.dsp", source);
    assert_eq!(text, repeated, "Cmajor table lowering leaked request state");
}

#[test]
fn one_sample_io_uses_cmajor_streams() {
    let text = cmajor("io.dsp", "process = _ , _ : +;");
    assert!(text.contains("input stream float32 input1;"), "{text}");
    assert!(text.contains("input stream float32 input0;"), "{text}");
    assert!(text.contains("output stream float32 output0;"), "{text}");
    assert!(text.contains("output0 <-"), "{text}");
    assert_eq!(text.matches("advance();").count(), 1, "{text}");
    assert!(!text.contains("inputs["), "{text}");
    assert!(!text.contains("outputs["), "{text}");
    assert_cmajor_frontend("io", &text);
}

#[test]
fn intrinsic_execution_flags_do_not_change_facade_output() {
    let source = "process = _ * hslider(\"gain\", 0.5, 0, 1, 0.01);";
    let options = CmajorOptions::default();
    let mut outputs = Vec::new();
    for (control, api) in [
        (ControlRateMode::InlinePerBlock, ProcessingApi::Block),
        (ControlRateMode::External, ProcessingApi::Block),
        (ControlRateMode::InlinePerBlock, ProcessingApi::OneSample),
        (ControlRateMode::External, ProcessingApi::OneSample),
    ] {
        outputs.push(
            Compiler::new()
                .with_control_rate_mode(control)
                .with_processing_api(api)
                .compile_source_to_cmajor("intrinsic.dsp", source, &options)
                .expect("intrinsic Cmajor modes must compile"),
        );
    }
    assert!(outputs.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn facade_and_cli_emit_identical_cmajor_source() {
    let root =
        std::env::temp_dir().join(format!("faust-rs-cmajor-cli-parity-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create Cmajor CLI test directory");
    let dsp = root.join("parity.dsp");
    std::fs::write(&dsp, "process = _ * hslider(\"gain\", 0.5, 0, 1, 0.01);\n")
        .expect("write Cmajor CLI source");

    let output = Command::new(env!("CARGO_BIN_EXE_faust-rs"))
        .arg("-lang")
        .arg("cmajor")
        .arg(&dsp)
        .output()
        .expect("run Cmajor CLI");
    assert!(
        output.status.success(),
        "Cmajor CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli = String::from_utf8(output.stdout).expect("Cmajor CLI output is UTF-8");
    let facade = Compiler::new()
        .compile_file_default_to_cmajor(&dsp, &CmajorOptions::default())
        .expect("Cmajor facade file compilation succeeds");
    assert_eq!(cli.trim(), facade.trim());
    assert_cmajor_frontend("cli-parity", &cli);

    std::fs::remove_file(&dsp).expect("remove Cmajor CLI source");
    std::fs::remove_dir(&root).expect("remove Cmajor CLI test directory");
}

#[test]
fn cli_honors_name_double_output_json_and_vector_rejection() {
    let root = std::env::temp_dir().join(format!(
        "faust-rs-cmajor-cli-options-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("create Cmajor CLI options directory");
    let dsp = root.join("options.dsp");
    let output_path = root.join("custom.cmajor");
    let json_path = output_path.with_extension("json");
    std::fs::write(&dsp, "process = _ * 0.5;\n").expect("write Cmajor CLI options source");

    let output = Command::new(env!("CARGO_BIN_EXE_faust-rs"))
        .args(["-lang", "cmajor", "-double", "-cn", "Custom", "--json"])
        .arg("-o")
        .arg(&output_path)
        .arg(&dsp)
        .output()
        .expect("run Cmajor CLI options");
    assert!(
        output.status.success(),
        "Cmajor CLI options failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cmajor = std::fs::read_to_string(&output_path).expect("read Cmajor output");
    assert!(cmajor.contains("processor Custom"), "{cmajor}");
    assert!(cmajor.contains("stream float64"), "{cmajor}");
    let json = std::fs::read_to_string(&json_path).expect("read Cmajor JSON companion");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid companion JSON");
    assert_eq!(parsed["inputs"], 1);
    assert_eq!(parsed["outputs"], 1);
    assert_cmajor_frontend("cli-options", &cmajor);

    let vector = Command::new(env!("CARGO_BIN_EXE_faust-rs"))
        .args(["-lang", "cmajor", "-vec"])
        .arg(&dsp)
        .output()
        .expect("run rejected Cmajor vector request");
    assert!(!vector.status.success());
    assert!(
        String::from_utf8_lossy(&vector.stderr).contains("FRS-EXEC-VEC-BACKEND")
            || String::from_utf8_lossy(&vector.stderr).contains("cannot be used"),
        "{}",
        String::from_utf8_lossy(&vector.stderr)
    );

    for path in [&dsp, &output_path, &json_path] {
        std::fs::remove_file(path).expect("remove Cmajor CLI options artifact");
    }
    std::fs::remove_dir(&root).expect("remove Cmajor CLI options directory");
}

#[test]
fn pinned_cpp_and_rust_observable_contracts_match() {
    let cases = [
        ("diff-io", "process = _ , _ : +;", false),
        (
            "diff-ui",
            "process = _ * hslider(\"gain[unit:dB]\", 0.5, 0, 1, 0.01);",
            false,
        ),
        (
            "diff-bargraph",
            "process = _ <: attach(_, vbargraph(\"lvl\", 0, 1));",
            false,
        ),
        ("diff-table", "process = rdtable(8, 0.25, int(_));", false),
        ("diff-double", "process = _ * 0.5;", true),
    ];
    for (name, source, double) in cases {
        let Some(cpp) = cpp_cmajor(name, source, double) else {
            return;
        };
        let options = CmajorOptions {
            real_type: if double {
                CmajorRealType::Float64
            } else {
                CmajorRealType::Float32
            },
            ..CmajorOptions::default()
        };
        let rust = cmajor_with(name, source, &options);
        assert_eq!(
            cmajor_contract(&rust),
            cmajor_contract(&cpp),
            "observable Cmajor contract drift for {name}\nRust:\n{rust}\nC++:\n{cpp}"
        );
    }
}

#[test]
fn cmajor_runtime_matches_expected_recurrence_at_o0_and_o4() {
    let Ok(cmaj) = std::env::var("CMAJ_BIN") else {
        return;
    };
    let root = std::env::temp_dir().join(format!("faust-rs-cmajor-runtime-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create Cmajor runtime directory");
    let mut source = cmajor("runtime.dsp", "process = 1 : + ~ *(0.5);");
    source.push_str(
        "\ngraph RuntimeMain [[ main ]]\n{\n\toutput stream float32 audioOut;\n\tnode dsp = faust::mydsp;\n\tconnection dsp.output0 -> audioOut;\n}\n",
    );
    std::fs::write(root.join("runtime.cmajor"), source).expect("write runtime Cmajor source");
    std::fs::write(
        root.join("runtime.cmajorpatch"),
        r#"{
  "CmajorVersion": 1,
  "ID": "dev.faust_rs.runtime",
  "version": "1.0",
  "name": "mydsp",
  "description": "Cmajor backend optimization parity fixture",
  "category": "synth",
  "manufacturer": "faust-rs",
  "isInstrument": false,
  "source": "runtime.cmajor"
}
"#,
    )
    .expect("write runtime Cmajor patch");

    let o0 = run_cmajor_generated_cpp(&cmaj, &root, "-O0");
    let o4 = run_cmajor_generated_cpp(&cmaj, &root, "-O4");
    assert_eq!(o0.len(), 16);
    assert_eq!(o4.len(), 16);
    for (index, (&unoptimized, &optimized)) in o0.iter().zip(&o4).enumerate() {
        assert!(
            (unoptimized - optimized).abs() <= 1.0e-6,
            "Cmajor optimization drift at sample {index}: O0={unoptimized}, O4={optimized}"
        );
    }
    for (index, &sample) in o0.iter().enumerate() {
        let expected = 2.0 - 2.0_f32.powi(-(index as i32));
        assert!(
            (sample - expected).abs() <= 1.0e-5,
            "unexpected recurrence at sample {index}: got {sample}, expected {expected}"
        );
    }

    std::fs::remove_dir_all(&root).expect("remove Cmajor runtime directory");
}

#[test]
fn recurrence_and_delay_emit_state_and_dynamic_access() {
    let recurrence = cmajor("rec.dsp", "process = + ~ *(0.5);");
    assert!(
        recurrence.contains("fRec") || recurrence.contains("fVec"),
        "{recurrence}"
    );
    assert!(recurrence.contains("loop"), "{recurrence}");
    assert_cmajor_frontend("recurrence", &recurrence);

    let delay = cmajor("delay.dsp", "process = _ : @(7);");
    assert!(delay.contains(".at("), "{delay}");
    assert!(delay.contains("output0 <-"), "{delay}");
    assert_cmajor_frontend("delay", &delay);
}

#[test]
fn scalar_math_names_are_accepted_by_cmajor() {
    let text = cmajor(
        "math.dsp",
        "process = (_ : sin), (_ : cos), (_ : abs), (_ : sqrt);",
    );
    for name in ["sin(", "cos(", "abs(", "sqrt("] {
        assert!(text.contains(name), "missing {name}: {text}");
    }
    assert_cmajor_frontend("math", &text);
}

#[test]
fn generated_lifecycle_obeys_backend_contract() {
    let text = cmajor("life.dsp", "process = + ~ *(0.5);");
    let instance = text
        .split("void instanceInit(int sample_rate)")
        .nth(1)
        .and_then(|tail| tail.split("void init()").next())
        .expect("instanceInit section");
    let constants = instance.find("instanceConstants").expect("constants");
    let reset = instance
        .find("instanceResetUserInterface")
        .expect("reset UI");
    let clear = instance.find("instanceClear").expect("clear");
    assert!(constants < reset && reset < clear, "{text}");
    assert!(!instance.contains("classInit"), "{text}");

    let init = text
        .split("void init()")
        .nth(1)
        .and_then(|tail| tail.split("void control()").next())
        .expect("init section");
    assert!(
        init.find("classInit(sample_rate)") < init.find("instanceInit(sample_rate)"),
        "{text}"
    );
}

#[test]
fn unsupported_soundfile_is_a_typed_error() {
    let compiler = Compiler::new()
        .with_control_rate_mode(ControlRateMode::External)
        .with_processing_api(ProcessingApi::OneSample);
    let fir = compiler
        .compile_source_to_fir_with_lane(
            "sound.dsp",
            "process = 0,0 : soundfile(\"sound[url:{'x.wav'}]\", 2) : !,!,_,_;",
            SignalFirLane::TransformFastLane,
        )
        .expect("soundfile reaches FIR so the backend can reject it");
    let error = generate_cmajor_module(&fir.store, fir.module, &CmajorOptions::default())
        .expect_err("Cmajor must reject soundfiles");
    assert_eq!(error.code, CodegenErrorCode::Unsupported);
    assert_eq!(error.code.as_str(), "FRS-CGEN-CMAJ-0002");
}
