//! Integration tests for zita-related signal/FIR pipeline regressions.

use compiler::{Compiler, CompilerError, SignalCompileOutput};
use parser::SourceReaderError;
use std::path::PathBuf;
use std::thread;
use transform::signal_fir::{
    RealType, SignalFirOptions, SignalFirRequest, compile_signals_to_fir_fastlane,
};
use transform::signal_prepare::prepare_signals_for_fir;
use ui::UiProgram;

const DEFAULT_FAUSTLIBRARIES_ROOT: &str = "/Users/letz/Developpements/faustlibraries";

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("zita")
}

fn fixture_dsp(file: &str) -> PathBuf {
    fixture_root().join(file)
}

fn faustlibraries_root() -> Option<PathBuf> {
    std::env::var_os("FAUST_RS_FAUSTLIBRARIES_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            let default = PathBuf::from(DEFAULT_FAUSTLIBRARIES_ROOT);
            default.exists().then_some(default)
        })
}

fn compile_zita_signals() -> Option<SignalCompileOutput> {
    let compiler = Compiler::new();
    let path = fixture_dsp("zita_min.dsp");

    if let Some(root) = faustlibraries_root() {
        return Some(
            compiler
                .compile_file_to_signals(&path, &[root])
                .unwrap_or_else(|e| panic!("signal compilation should succeed: {e}")),
        );
    }

    match compiler.compile_file_default_to_signals(&path) {
        Ok(output) => Some(output),
        Err(CompilerError::Import(error, _))
            if matches!(
                error.as_ref(),
                SourceReaderError::UnresolvedImport { name, .. } if name.as_ref() == "stdfaust.lib"
            ) =>
        {
            eprintln!(
                "Skipping zita pipeline: stdfaust.lib unavailable and FAUST_RS_FAUSTLIBRARIES_ROOT is unset"
            );
            None
        }
        Err(e) => panic!("signal compilation should succeed: {e}"),
    }
}

fn run_with_large_stack<T>(f: impl FnOnce() -> T + Send + 'static) -> T
where
    T: Send + 'static,
{
    thread::Builder::new()
        .name("zita-pipeline".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(f)
        .expect("spawn zita worker thread")
        .join()
        .expect("zita worker thread should finish")
}

#[test]
fn zita_min_preparation_preserves_multi_output_recursion_groups() {
    run_with_large_stack(|| {
        let Some(output) = compile_zita_signals() else {
            return;
        };
        let prepared = prepare_signals_for_fir(
            &output.parse.state.arena,
            &output.signals,
            &UiProgram::empty(),
        )
        .expect("signal preparation should succeed for zita_min");
        assert_eq!(prepared.outputs().len(), 2);
    });
}

#[test]
fn zita_min_fastlane_fir_lowering_completes() {
    run_with_large_stack(|| {
        let Some(output) = compile_zita_signals() else {
            return;
        };
        let fir = compile_signals_to_fir_fastlane(&SignalFirRequest::new(
            &output.parse.state.arena,
            &output.signals,
            output.process_arity.inputs,
            output.propagated_output_count(),
            &UiProgram::empty(),
            &SignalFirOptions {
                module_name: "mydsp".to_owned(),
                real_type: RealType::Float32,
                ..SignalFirOptions::default()
            },
        ))
        .expect("fast-lane FIR lowering should succeed for zita_min");
        assert!(fir.module.as_u32() > 0);
    });
}
