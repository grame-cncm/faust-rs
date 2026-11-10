//! One `rdtable` read from several `ondemand` bodies is one table.
//!
//! Propagation annotates the first element of a `waveform` with the domain it
//! is written in (C++ `boxWaveform`), so the generator of a table read inside
//! two clock domains used to be two distinct trees, and every backend emitted
//! one generator sub-module, one waveform literal and one table per domain.
//! The faust-coopiano voice read its two 12001-point bridge-admittance tables
//! from 24 domains each: 73 table sub-modules where 4 tables exist, and
//! 18.5 MB of C++ where 4.35 MB remain once they are shared.
//!
//! Both table-fill paths drop those annotations before computing the content,
//! so the lowerers now declare a read-only table once per content modulo the
//! annotations (`transform::signal_fir::table_content`) and every domain reads
//! it. Rust-only: the C++ reference keeps one table per domain (`DIFF-BEH-029`
//! in `porting/faust-rs-vs-faust-cpp-differences-en.md`).
//!
//! A generator with state (`ba.time` in the table of `os.osc`) is one
//! recursion group per domain, each with a variable of its own: the content is
//! also compared modulo the names of the variables a generator binds, so that
//! table is shared like a waveform.
//!
//! The structural tests pin the count on every text backend with tables and on
//! the wasm data segments; the negative controls pin what must *not* merge
//! (different content, writable tables); the execution tests check the shared
//! table still gives each domain its own reads.

use std::io::Cursor;

use codegen::backends::c::COptions;
use codegen::backends::cmajor::CmajorOptions;
use codegen::backends::cpp::CppOptions;
use codegen::backends::interp::{FbcDspInstance, InterpOptions, MAX_OPT_LEVEL, read_fbc};
use codegen::backends::julia::JuliaOptions;
use codegen::backends::mojo::MojoOptions;
use codegen::backends::rust::RustOptions;
use codegen::backends::wasm::WasmOptions;
use compiler::{Compiler, ComputeMode, TableInitMode};

/// The reproducer: one table, read by two `ondemand` bodies.
const SHARED: &str = "w = waveform{0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8};\n\
rd(i) = w, int(i) : rdtable;\n\
x = hslider(\"x\", 0, 0, 6, 1);\n\
y = hslider(\"y\", 0, 0, 6, 1);\n\
cx = x != x';\n\
cy = y != y';\n\
process = ((cx, x) : ondemand(rd)) + ((cy, y) : ondemand(\\(v).(rd(v + 1))));\n";

const WAVE: [f32; 8] = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];

/// Number of distinct generator sub-modules `{module}SIG{k}` named in `text`
/// (case-insensitive, so the Mojo `mydsp_sig{k}` spelling counts too).
fn sub_modules(text: &str) -> usize {
    let lower = text.to_ascii_lowercase().replace("mydsp_sig", "mydspsig");
    let mut names = std::collections::BTreeSet::new();
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("mydspsig") {
        let tail = &rest[at + "mydspsig".len()..];
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            names.insert(digits);
        }
        rest = tail;
    }
    names.len()
}

fn cpp(source: &str) -> String {
    Compiler::new()
        .compile_source_to_cpp("shared", source, &CppOptions::default())
        .expect("C++ codegen")
}

/// `source` on the six text backends that have tables.
fn text_backends(source: &str) -> [(&'static str, String); 6] {
    let c = Compiler::new();
    [
        ("cpp", cpp(source)),
        (
            "c",
            c.compile_source_to_c("shared", source, &COptions::default())
                .expect("C codegen"),
        ),
        (
            "rust",
            c.compile_source_to_rust("shared", source, &RustOptions::default())
                .expect("Rust codegen"),
        ),
        (
            "julia",
            c.compile_source_to_julia("shared", source, &JuliaOptions::default())
                .expect("Julia codegen"),
        ),
        (
            "mojo",
            c.compile_source_to_mojo("shared", source, &MojoOptions::default())
                .expect("Mojo codegen"),
        ),
        (
            "cmajor",
            c.compile_source_to_cmajor("shared", source, &CmajorOptions::default())
                .expect("Cmajor codegen"),
        ),
    ]
}

#[test]
fn one_table_read_from_two_domains_is_one_sub_module_on_every_text_backend() {
    let outputs = text_backends(SHARED);
    for (lang, text) in &outputs {
        assert_eq!(
            sub_modules(text),
            1,
            "{lang}: the table read from two ondemand bodies must be generated once\n{text}"
        );
    }
    let cpp = &outputs[0].1;
    assert_eq!(cpp.matches("static float ftbl").count(), 1, "{cpp}");
    assert_eq!(cpp.matches("Wave0[8] = {").count(), 1, "{cpp}");
    // Both domains read the one table.
    assert_eq!(cpp.matches("= ftbl0mydspSIG0[").count(), 2, "{cpp}");
}

#[test]
fn one_table_read_from_two_domains_is_one_wasm_data_segment() {
    let module = Compiler::new()
        .compile_source_to_wasm("shared", SHARED, &WasmOptions::default())
        .expect("wasm codegen");
    let literal: Vec<u8> = WAVE.iter().flat_map(|v| v.to_le_bytes()).collect();
    let copies = module
        .wasm_binary
        .windows(literal.len())
        .filter(|window| *window == literal.as_slice())
        .count();
    assert_eq!(copies, 1, "the waveform literal must be emitted once");
}

/// Under `-vec` these programs fall back to scalar lowering today ("loop
/// belongs to more than one clock island"); the checked vector lowerer keys
/// its tables the same way for the day they are admitted.
#[test]
fn the_sharing_holds_in_vector_and_const_modes() {
    let vector = Compiler::new()
        .with_compute_mode(ComputeMode::Vector {
            vec_size: 32,
            loop_variant: 0,
        })
        .compile_source_to_cpp("shared", SHARED, &CppOptions::default())
        .expect("vector C++ codegen");
    assert_eq!(sub_modules(&vector), 1, "{vector}");

    // `--table-init const` folds the table: no sub-module, one literal table.
    let folded = Compiler::new()
        .with_table_init_mode(TableInitMode::Const)
        .compile_source_to_cpp("shared", SHARED, &CppOptions::default())
        .expect("const C++ codegen");
    assert_eq!(sub_modules(&folded), 0, "{folded}");
    assert_eq!(folded.matches("static float ftbl").count(), 1, "{folded}");
}

#[test]
fn tables_of_different_content_stay_distinct() {
    let source = "w = waveform{0.1, 0.2, 0.3, 0.4};\n\
v = waveform{0.5, 0.6, 0.7, 0.8};\n\
x = hslider(\"x\", 0, 0, 3, 1);\n\
cx = x != x';\n\
process = ((cx, x) : ondemand(\\(i).(w, int(i) : rdtable))) \
+ ((cx, x) : ondemand(\\(i).(v, int(i) : rdtable)));\n";
    let text = cpp(source);
    assert_eq!(sub_modules(&text), 2, "{text}");
}

#[test]
fn writable_tables_in_two_domains_keep_their_own_state() {
    // Same generator, same write signals up to the domain: each body owns its
    // table, which it writes in its own fire time.
    let source = "x = hslider(\"x\", 0, 0, 3, 1);\n\
cx = x != x';\n\
t(i) = rwtable(4, 0.0, int(i), float(i), int(i));\n\
process = ((cx, x) : ondemand(t)) + ((cx, x) : ondemand(\\(i).(t(i + 1))));\n";
    let text = cpp(source);
    assert_eq!(text.matches("float ftbl").count(), 2, "{text}");
}

/// `n` counts samples; body 1 fires on even samples and reads `w[n % 7]`,
/// body 2 fires every third sample and reads `w[n % 5 + 1]`. Each body holds
/// its last read between fires.
const TIMED: &str = "w = waveform{0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8};\n\
rd(i) = w, int(i) : rdtable;\n\
n = (+(1) ~ _) - 1;\n\
process = ((n % 2 == 0, n % 7) : ondemand(rd)), \
((n % 3 == 0, n % 5) : ondemand(\\(v).(rd(v + 1))));\n";

fn expected(t: usize) -> (f32, f32) {
    let even = t - t % 2;
    let third = t - t % 3;
    (WAVE[even % 7], WAVE[third % 5 + 1])
}

#[test]
fn the_timed_program_shares_its_table() {
    assert_eq!(sub_modules(&cpp(TIMED)), 1);
}

/// `frames` frames of each of the `outputs` outputs of `source` through the
/// interpreter, in scalar and vector mode, unoptimized and fully optimized.
fn for_each_interp_run(
    source: &str,
    outputs: usize,
    frames: usize,
    mut check: impl FnMut(&str, &[Vec<f32>]),
) {
    let modes = [
        ComputeMode::Scalar,
        ComputeMode::Vector {
            vec_size: 32,
            loop_variant: 0,
        },
    ];
    for mode in modes {
        for opt_level in [0, i32::from(MAX_OPT_LEVEL)] {
            let fbc = Compiler::new()
                .with_compute_mode(mode)
                .compile_source_to_interp(
                    "timed",
                    source,
                    &InterpOptions {
                        opt_level,
                        ..Default::default()
                    },
                )
                .expect("interpreter codegen");
            let mut factory = read_fbc::<f32>(&mut Cursor::new(fbc)).expect("read FBC");
            let mut instance = FbcDspInstance::new(&mut factory);
            instance.init(44100);
            let mut channels = vec![vec![f32::NAN; frames]; outputs];
            let mut slices: Vec<&mut [f32]> = channels.iter_mut().map(Vec::as_mut_slice).collect();
            instance
                .try_compute(
                    i32::try_from(frames).expect("frame count"),
                    &[],
                    &mut slices,
                )
                .expect("compute");
            check(&format!("{mode:?} opt={opt_level}"), &channels);
        }
    }
}

/// Numeric only, so it holds with and without the sharing: the output is the
/// same, only the number of copies of the table changes.
#[test]
fn each_domain_reads_the_shared_table_in_its_own_fire_time() {
    for_each_interp_run(TIMED, 2, 300, |run, out| {
        for (t, (left, right)) in out[0].iter().zip(&out[1]).enumerate() {
            let (l, r) = expected(t);
            assert_eq!(
                (left.to_bits(), right.to_bits()),
                (l.to_bits(), r.to_bits()),
                "{run} sample {t}: got ({left}, {right}), expected ({l}, {r})"
            );
        }
    });
}

/// A generator with state: `(+(3) ~ _) - 3` fills the table with 0, 3, 6, ...
/// as `ba.time` drives the table of `os.osc`. Its recursion is one group per
/// domain that reads the table, each with its own variable. Body 1 fires on
/// even samples and reads `ramp[n % 7]`, body 2 every third sample and reads
/// `ramp[n % 5 + 1]`, the top level reads `ramp[n % 8]` at every sample.
const RECURSIVE: &str = "ramp = (+(3) ~ _) - 3;\n\
rd(i) = rdtable(8, ramp, int(i));\n\
n = (+(1) ~ _) - 1;\n\
process = ((n % 2 == 0, n % 7) : ondemand(rd)), \
((n % 3 == 0, n % 5) : ondemand(\\(v).(rd(v + 1)))), \
rd(n % 8);\n";

#[test]
fn a_stateful_generator_read_from_three_domains_is_one_sub_module() {
    for (lang, text) in &text_backends(RECURSIVE) {
        assert_eq!(
            sub_modules(text),
            1,
            "{lang}: the generator's recursion is one group per domain, the table one\n{text}"
        );
    }
    // A real generator of the same shape, the one of `os.osc`.
    let sine = "t = (+(1) ~ _) - 1;\n\
osc(i) = rdtable(64, sin(2 * 3.14159265 * float(t) / 64), int(i) % 64);\n\
n = (+(1) ~ _) - 1;\n\
process = ((n % 2 == 0, n) : ondemand(osc)), ((n % 3 == 0, n) : ondemand(\\(v).(osc(v + 1)))), osc(n);\n";
    assert_eq!(sub_modules(&cpp(sine)), 1);
    for mode in [
        ComputeMode::Vector {
            vec_size: 32,
            loop_variant: 0,
        },
        ComputeMode::Scalar,
    ] {
        let text = Compiler::new()
            .with_compute_mode(mode)
            .with_table_init_mode(TableInitMode::Const)
            .compile_source_to_cpp("shared", RECURSIVE, &CppOptions::default())
            .expect("const C++ codegen");
        assert_eq!(
            text.matches("static int itbl").count(),
            1,
            "{mode:?}\n{text}"
        );
    }
}

#[test]
fn stateful_generators_of_different_content_stay_distinct() {
    let source = "by(k) = (+(k) ~ _) - k;\n\
x = hslider(\"x\", 0, 0, 3, 1);\n\
cx = x != x';\n\
process = ((cx, x) : ondemand(\\(i).(rdtable(8, by(3), int(i))))) \
+ ((cx, x) : ondemand(\\(i).(rdtable(8, by(5), int(i)))));\n";
    let text = cpp(source);
    assert_eq!(sub_modules(&text), 2, "{text}");
}

/// Numeric only, as above: the samples are the same with one table or three.
#[test]
fn each_domain_reads_the_shared_stateful_table_in_its_own_fire_time() {
    for_each_interp_run(RECURSIVE, 3, 300, |run, out| {
        let samples = out[0].iter().zip(&out[1]).zip(&out[2]).enumerate();
        for (t, ((&first, &second), &top)) in samples {
            let even = t - t % 2;
            let third = t - t % 3;
            let want = [3 * (even % 7), 3 * (third % 5 + 1), 3 * (t % 8)];
            let got = [first, second, top];
            #[allow(clippy::cast_precision_loss)]
            let want = want.map(|v| v as f32);
            assert_eq!(got, want, "{run} sample {t}");
        }
    });
}
