//! Regression coverage for the persisted interpreter-bytecode fixture used by
//! the `--dump-cpp-from-fbc` CLI mode.

use std::io::BufReader;

use codegen::backends::interp::{FbcCppOptions, generate_cpp_from_fbc, read_fbc};

const ONDEMAND_STFT_COLA_016_FBC: &str = include_str!("fixtures/interp/ondemand_stft_cola_016.fbc");

#[test]
fn ondemand_stft_fixture_does_not_emit_an_empty_else_branch() {
    let mut reader = BufReader::new(ONDEMAND_STFT_COLA_016_FBC.as_bytes());
    let factory = read_fbc::<f32>(&mut reader).expect("fixture should deserialize");
    let cpp = generate_cpp_from_fbc(&factory, &FbcCppOptions::default())
        .expect("fixture should generate C++");

    assert!(
        !cpp.contains("} else {"),
        "empty C++ else branch must not be emitted:\n{cpp}"
    );
}
