//! Integer pow must also build inside the host-supplied FaustDsp contract.
//! The single-precision module still needs the double rint bridge (#1340).

use codegen::backends::rust::{RustOptions, RustRealType};
use compiler::{Compiler, RealType};
use std::process::Command;

#[test]
fn generated_rust_integer_pow_builds_and_runs_at_both_precisions() {
    let architecture = include_str!("../../../tests/impulse-tests/archs/impulserust.rs");
    let prefix = architecture
        .split("const SAMPLE_RATE:")
        .next()
        .expect("host API");
    let source = "process(x) = pow(250,x<1)-(1+249*(x<1)), pow(25,6*(x<1))-(1+244140624*(x<1)), pow(-3,3*(x<1))-(1-28*(x<1));";
    let dir =
        std::env::temp_dir().join(format!("faust-rs-integer-pow-rust-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp directory");
    for double in [false, true] {
        let prefix = if double {
            prefix.to_owned()
        } else {
            prefix
                .replace("pub type F32 = f64;", "pub type F32 = f32;")
                .replace("pub type FaustFloat = F64;", "pub type FaustFloat = F32;")
        };
        let generated = Compiler::new()
            .with_real_type(if double {
                RealType::Float64
            } else {
                RealType::Float32
            })
            .compile_source_to_rust(
                "integer-pow",
                source,
                &RustOptions {
                    faust_float_type: if double {
                        RustRealType::Float64
                    } else {
                        RustRealType::Float32
                    },
                    ..Default::default()
                },
            )
            .expect("Rust codegen");
        let main = r#"
fn main() {
    let mut d = mydsp::new(); d.init(44100);
    let input: [FaustFloat; 2] = [0.0,1.0];
    let mut a = [FaustFloat::NAN; 2];
    let mut b = [FaustFloat::NAN; 2];
    let mut c = [FaustFloat::NAN; 2];
    d.compute(2, &[&input], &mut [&mut a, &mut b, &mut c]);
    assert_eq!(a, [0.0; 2]); assert_eq!(b, [0.0; 2]); assert_eq!(c, [0.0; 2]);
}
"#;
        let file = dir.join("runner.rs");
        std::fs::write(&file, format!("{prefix}\n{generated}\n{main}")).expect("write runner");
        for opt in ["0", "3"] {
            let exe = dir.join(format!("runner{}", std::env::consts::EXE_SUFFIX));
            let built = Command::new("rustc")
                .arg("--edition=2024")
                .arg("-C")
                .arg(format!("opt-level={opt}"))
                .arg(&file)
                .arg("-o")
                .arg(&exe)
                .output()
                .expect("rustc");
            assert!(
                built.status.success(),
                "{}",
                String::from_utf8_lossy(&built.stderr)
            );
            let run = Command::new(&exe).output().expect("run Rust DSP");
            assert!(
                run.status.success(),
                "{}",
                String::from_utf8_lossy(&run.stderr)
            );
        }
    }
    std::fs::remove_dir_all(dir).expect("cleanup");
}
