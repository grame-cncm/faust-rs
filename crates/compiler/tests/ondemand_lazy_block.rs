//! Block-rate work inside a boolean `ondemand` guard is computed at the first
//! fire of each block, in one prologue per guard, instead of before the sample
//! loop (`porting/ondemand-block-rate-in-domain-analysis-and-plan-2026-10-17-en.md`;
//! registry DIFF-BEH-024).
//!
//! The DSPs are self-contained: no standard library import.

use std::collections::HashSet;

use codegen::backends::cpp::CppOptions;
use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, ComputeMode, ControlRateMode, SignalFirLane};
use fir::{AccessType, FirId, FirMatch, FirStore, fir_match_children, match_fir};

/// A control-rate input and a payload of transcendental functions of it, the
/// shape of faust-coopiano's `on_change` parameters (RAPPORT.md, point 6).
const HEAVY: &str = r#"
    m = hslider("m", 60, 0, 127, 1);
    heavy(x) = exp(-0.001 * x) * cos(0.01 * x) + exp(-0.002 * x) * cos(0.02 * x)
             + exp(-0.003 * x) * cos(0.03 * x);
"#;

fn source(process: &str) -> String {
    format!("{HEAVY}\nprocess = {process};")
}

fn cpp(compiler: &Compiler, process: &str) -> String {
    compiler
        .compile_source_to_cpp_with_lane(
            "ondemand_lazy_block",
            &source(process),
            &CppOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|error| panic!("`{process}` must compile: {error}"))
}

/// `(exp calls before the sample loop, exp calls in it)` of a C++ `compute`.
fn exp_split(cpp: &str) -> (usize, usize) {
    let compute = &cpp[cpp.find("virtual void compute(").expect("compute")..];
    let loop_start = compute.find("for (int i0").expect("sample loop");
    let (before, inside) = compute.split_at(loop_start);
    (
        before.matches("std::exp(").count(),
        inside.matches("std::exp(").count(),
    )
}

/// What [`check_prologues`] counted in `compute`.
#[derive(Debug, PartialEq)]
struct Prologues {
    /// Guards with a lazy prologue.
    prologues: usize,
    /// Lazy values the rest of their guard reads: declared, zeroed, before the
    /// sample loop and stored by the prologue.
    kept: usize,
    /// Lazy values only later prologue entries read: locals of the prologue.
    locals: usize,
}

/// Checks the prologue contract on the FIR of `compute`: each read of a lazy
/// value (`*SlowOD*`) happens after it is computed, on a path that went through
/// the prologue of its own guard, and a prologue local is read only inside its
/// prologue; each guard holds at most one prologue, as its first statement.
fn check_prologues(store: &FirStore, module: FirId) -> Prologues {
    let compute = find_compute(store, module).expect("compute body");
    let mut checker = PrologueChecker {
        store,
        available: HashSet::new(),
        prologue_locals: None,
        counts: Prologues {
            prologues: 0,
            kept: 0,
            locals: 0,
        },
    };
    checker.statement(compute);
    checker.counts
}

fn is_lazy(name: &str) -> bool {
    name.contains("SlowOD") && !name.contains("SlowODDone")
}

fn find_compute(store: &FirStore, id: FirId) -> Option<FirId> {
    if let FirMatch::DeclareFun {
        name,
        body: Some(body),
        ..
    } = match_fir(store, id)
        && name == "compute"
    {
        return Some(body);
    }
    fir_match_children(store, id)
        .into_iter()
        .find_map(|child| find_compute(store, child))
}

struct PrologueChecker<'a> {
    store: &'a FirStore,
    available: HashSet<String>,
    /// The locals of the prologue being walked, if any.
    prologue_locals: Option<Vec<String>>,
    counts: Prologues,
}

impl PrologueChecker<'_> {
    /// The flag a prologue tests (`if (iSlowODDone<n> == 0)`), if `id` is one.
    fn prologue_flag(&self, id: FirId) -> Option<String> {
        let FirMatch::If { cond, .. } = match_fir(self.store, id) else {
            return None;
        };
        fir_match_children(self.store, cond)
            .into_iter()
            .find_map(|c| match match_fir(self.store, c) {
                FirMatch::LoadVar { name, .. } if name.contains("SlowODDone") => Some(name),
                _ => None,
            })
    }

    fn statement(&mut self, id: FirId) {
        match match_fir(self.store, id) {
            FirMatch::Block(items) => {
                let scope = self.available.clone();
                for (index, item) in items.iter().enumerate() {
                    if self.prologue_flag(*item).is_some() {
                        assert_eq!(index, 0, "a lazy prologue must open its guard");
                        self.counts.prologues += 1;
                    }
                    self.statement(*item);
                }
                // A guard's lazy values are not visible after it closes.
                self.available = scope;
            }
            FirMatch::StoreVar {
                name,
                access: AccessType::Stack,
                value,
            } => {
                self.expression(value);
                if is_lazy(&name) {
                    self.available.insert(name);
                }
            }
            FirMatch::DeclareVar { name, init, .. } => {
                if let Some(init) = init {
                    self.expression(init);
                }
                assert!(
                    !self.available.contains(&name),
                    "lazy value `{name}` declared twice"
                );
                if is_lazy(&name) {
                    match self.prologue_locals.as_mut() {
                        // A prologue local is computed by its declaration.
                        Some(locals) => {
                            locals.push(name.clone());
                            self.available.insert(name);
                            self.counts.locals += 1;
                        }
                        // The zeroed declaration before the loop is not a
                        // computation.
                        None => self.counts.kept += 1,
                    }
                }
            }
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                self.expression(cond);
                // A prologue's stores stay visible to the rest of its guard;
                // its locals do not.
                if self.prologue_flag(id).is_some() {
                    let FirMatch::Block(items) = match_fir(self.store, then_block) else {
                        panic!("a prologue body is a block");
                    };
                    let outer = self.prologue_locals.replace(Vec::new());
                    assert!(outer.is_none(), "a prologue holds no prologue");
                    for item in items {
                        self.statement(item);
                    }
                    for local in self.prologue_locals.take().expect("prologue locals") {
                        self.available.remove(&local);
                    }
                } else {
                    self.statement(then_block);
                    if let Some(else_block) = else_block {
                        self.statement(else_block);
                    }
                }
            }
            _ => {
                for child in fir_match_children(self.store, id) {
                    match match_fir(self.store, child) {
                        FirMatch::Block(_)
                        | FirMatch::StoreVar { .. }
                        | FirMatch::DeclareVar { .. }
                        | FirMatch::If { .. } => self.statement(child),
                        _ => self.expression(child),
                    }
                }
            }
        }
    }

    fn expression(&mut self, id: FirId) {
        if let FirMatch::LoadVar { name, .. } = match_fir(self.store, id)
            && is_lazy(&name)
        {
            assert!(
                self.available.contains(&name),
                "lazy value `{name}` read before its prologue computed it"
            );
        }
        for child in fir_match_children(self.store, id) {
            self.expression(child);
        }
    }
}

#[test]
fn boolean_ondemand_block_rate_payload_runs_in_one_guard_prologue() {
    let out = cpp(&Compiler::new(), "(m != m', m) : ondemand(heavy)");
    assert_eq!(
        exp_split(&out),
        (0, 3),
        "the payload must leave the block prologue for the guard:\n{out}"
    );
    assert_eq!(
        out.matches("int iSlowODDone").count(),
        1,
        "one flag per guard"
    );
    assert!(out.contains("== 0) {"), "the prologue tests its flag");

    let fir = Compiler::new()
        .compile_source_to_fir_with_lane(
            "ondemand_lazy_block",
            &source("(m != m', m) : ondemand(heavy)"),
            SignalFirLane::TransformFastLane,
        )
        .expect("FIR");
    assert_eq!(
        check_prologues(&fir.store, fir.module),
        Prologues {
            prologues: 1,
            kept: 1,
            locals: 0
        }
    );
}

#[test]
fn several_lazy_values_of_one_guard_share_its_flag() {
    // `heavy(x)` is shared, so it is a lazy value of its own, as is the held
    // sum. A value the rest of the guard reads stays live across the sample
    // loop; one only the prologue reads is a local of the prologue.
    for (process, kept, locals) in [
        // The guard reads only the sum.
        (
            "(m != m', m) : ondemand(\\(x).(heavy(x) + sin(heavy(x))))",
            1,
            1,
        ),
        // The guard also holds `heavy(x)` itself.
        (
            "(m != m', m) : ondemand(\\(x).(heavy(x), heavy(x) + sin(heavy(x))))",
            2,
            0,
        ),
    ] {
        let out = cpp(&Compiler::new(), process);
        assert_eq!(
            out.matches("int iSlowODDone").count(),
            1,
            "one flag:\n{out}"
        );
        assert_eq!(
            exp_split(&out),
            (0, 3),
            "`heavy` is computed once, in the prologue:\n{out}"
        );
        let fir = Compiler::new()
            .compile_source_to_fir_with_lane(
                "ondemand_lazy_block",
                &source(process),
                SignalFirLane::TransformFastLane,
            )
            .expect("FIR");
        assert_eq!(
            check_prologues(&fir.store, fir.module),
            Prologues {
                prologues: 1,
                kept,
                locals
            },
            "`{process}`"
        );
    }
}

#[test]
fn nested_boolean_guards_each_get_their_prologue() {
    let process = "(m != m', m) : ondemand(\\(x).((x > 10, x) : ondemand(heavy) + exp(x)))";
    let out = cpp(&Compiler::new(), process);
    assert_eq!(
        exp_split(&out).0,
        0,
        "nothing hoisted before the loop:\n{out}"
    );
    assert_eq!(
        out.matches("int iSlowODDone").count(),
        2,
        "one flag per guard:\n{out}"
    );
    let fir = Compiler::new()
        .compile_source_to_fir_with_lane(
            "ondemand_lazy_block",
            &source(process),
            SignalFirLane::TransformFastLane,
        )
        .expect("FIR");
    assert_eq!(check_prologues(&fir.store, fir.module).prologues, 2);
}

#[test]
fn a_value_also_needed_outside_the_guard_stays_hoisted() {
    // `heavy(m)` is read by the guard's payload and by the second output:
    // computing it lazily as well would compute it twice per block.
    let process = "(m != m', m) : ondemand(\\(x).(x + heavy(m))), heavy(m)";
    let out = cpp(&Compiler::new(), process);
    assert_eq!(exp_split(&out), (3, 0), "`heavy(m)` once, hoisted:\n{out}");
    let fir = Compiler::new()
        .compile_source_to_fir_with_lane(
            "ondemand_lazy_block",
            &source(process),
            SignalFirLane::TransformFastLane,
        )
        .expect("FIR");
    // Only the sum, which nothing outside the guard reads, is lazy.
    assert_eq!(
        check_prologues(&fir.store, fir.module),
        Prologues {
            prologues: 1,
            kept: 1,
            locals: 0
        }
    );
}

#[test]
fn counted_upsampled_and_decimated_domains_are_not_lazy() {
    // Integer ondemand runs its body several times per sample; downsampling
    // keeps the hoisting by decision (plan, D2).
    for process in ["(2, m) : ondemand(heavy)", "(4, m) : downsampling(heavy)"] {
        let out = cpp(&Compiler::new(), process);
        assert!(
            !out.contains("SlowOD"),
            "`{process}` must not be lazy:\n{out}"
        );
        assert_eq!(
            exp_split(&out),
            (3, 0),
            "`{process}` keeps the hoisting:\n{out}"
        );
    }
    // Upsampling zero-stuffs its input, so its payload is sample-rate anyway.
    let out = cpp(&Compiler::new(), "(2, m) : upsampling(heavy)");
    assert!(
        !out.contains("SlowOD"),
        "upsampling must not be lazy:\n{out}"
    );
}

#[test]
fn external_control_keeps_the_block_rate_hoisting() {
    // Under `-ec` the control section moves to control() (plan, D3).
    let compiler = Compiler::new().with_control_rate_mode(ControlRateMode::External);
    let out = cpp(&compiler, "(m != m', m) : ondemand(heavy)");
    assert!(!out.contains("SlowOD"), "`-ec` must not be lazy:\n{out}");
}

#[test]
fn vector_mode_gets_the_same_prologue() {
    for loop_variant in [0, 1] {
        let compiler = Compiler::new().with_compute_mode(ComputeMode::Vector {
            vec_size: 32,
            loop_variant,
        });
        let out = cpp(&compiler, "(m != m', m) : ondemand(heavy)");
        assert_eq!(
            out.matches("int iSlowODDone").count(),
            1,
            "-vec -lv {loop_variant}:\n{out}"
        );
        let fir = compiler
            .compile_source_to_fir_with_lane(
                "ondemand_lazy_block",
                &source("(m != m', m) : ondemand(heavy)"),
                SignalFirLane::TransformFastLane,
            )
            .expect("FIR");
        assert_eq!(check_prologues(&fir.store, fir.module).prologues, 1);
    }
}

/// Renders `frames` samples of a one-output program in the interpreter.
fn render(process: &str, frames: usize) -> Vec<f32> {
    let fbc = Compiler::new()
        .compile_source_to_interp(
            "ondemand_lazy_block",
            &source(process),
            &InterpOptions::default(),
        )
        .unwrap_or_else(|error| panic!("`{process}` must compile: {error}"));
    let mut reader = std::io::Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader).expect("bytecode");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    let mut out = vec![0.0_f32; frames];
    let mut outputs: [&mut [f32]; 1] = [&mut out];
    instance
        .try_compute(frames as i32, &[], &mut outputs)
        .expect("compute");
    out
}

#[test]
fn the_lazy_value_is_the_hoisted_value() {
    // The guard fires at the first sample (m != m' against the zero initial
    // state) and holds heavy(m); the hoisted program computes heavy(m) before
    // the loop. A block-rate value is constant within a block: same bits.
    let lazy = render("(m != m', m) : ondemand(heavy)", 256);
    let hoisted = render("heavy(m)", 256);
    assert!(lazy[0] != 0.0, "the guard fires at the first sample");
    assert_eq!(lazy, hoisted);
}
