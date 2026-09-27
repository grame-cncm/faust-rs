# Embedding faust-rs: the API layers, and which one to bind

faust-rs is written in Rust from end to end. Its C API is Rust code exported
with C linkage: it mirrors the libfaust C API, so a C or C++ host can link
`libfaust-rs` where it linked libfaust. A host reaches the compiler through
one of four layers, and the right one depends on the language the host, or
the binding, is written in.

The description of the layers and of the trade-off comes from shakfu, author
of `py-faust-rs`, in
[issue #17](https://github.com/grame-cncm/faust-rs/issues/17#issuecomment-5856868752).

## The four layers

```text
4  bindings to other languages   py-faust-rs (PyO3), ...         outside the workspace
3  Rust API                      faust                           safe Rust, no raw pointer
2  C API                         interp-ffi, cranelift-ffi,      extern "C", raw pointers
                                 box-ffi, signal-ffi,
                                 libfaust-ffi
                                 -> libfaust-rs (faust-ffi)
                                 wasm-ffi (faustwasm)
1  compiler and backends         parser ... codegen, compiler    pure Rust
```

1. **Compiler and backends.** The pipeline from `parser` to `codegen`, and the
   `compiler` crate that drives it (and hosts the `faust-rs` command line).
   Pure Rust, with no stability promise: these crates change as the port
   requires.
2. **C API.** Rust functions with C signatures (`extern "C"`, raw pointers,
   `UIGlue` and `MetaGlue` callback tables): the Interpreter and Cranelift
   factory and instance APIs, the Box and Signal APIs, and the
   backend-agnostic `expandDSP*` / `generateAuxFiles*` / `generateSHA1`.
   `faust-ffi` packages them as one library, `libfaust-rs`, with C and C++
   headers. `wasm-ffi` is the counterpart for `faustwasm`: a raw WASM ABI
   around the same compiler.
3. **Rust API.** The `faust` crate: a `Factory` is a compiled program, a
   `Dsp` an instance, controls are addressed by their `/group/label` paths.
   It calls layer 2 directly, as Rust functions, with no shared library in
   between, so a `Factory` has exactly the lifecycle of the C API (programs
   shared by SHA key, reference counted). All the `unsafe` of the path lives
   in this crate; its host writes none.
4. **Bindings to other languages.** For instance `py-faust-rs`, which exposes
   layer 3 to Python through PyO3.

Layers 2 and 3 are the two contracts of faust-rs. Layer 1 may change without
notice.

`cargo run -p xtask -- ffi-boundary-check` enforces the dependency direction
between them, under the names of the workspace: layer 1 is the *core*, the
crates of layer 2 are the *adapters*, and `faust-ffi`, `wasm-ffi` and `faust`
are the *distribution* crates, what a host links. No crate depends on a layer
to its right, and only the adapters, the distribution crates that need it and
the `foreign-call` runtime bridge may opt into `unsafe`.

## Layer 3 in the code

The Rust API is the `faust` crate, `crates/faust`. Its public surface is what
`src/lib.rs` defines and re-exports; the rest is private.

| File | Holds |
| --- | --- |
| `src/lib.rs` | the crate documentation (model, precision, lifecycle, known gap); `Backend`, `Precision`, `CompileOptions`, `Error`, `ErrorKind`, `version()`; the re-exports |
| `src/factory.rs` | `Factory`: `from_file`, `from_source`, `instantiate`, `json`, `name`, `backend`, `precision` |
| `src/dsp.rs` | `Dsp`: `compute_f32`, `compute_f64`, `controls`, `control`, `get`, `set`, `metadata`, the initialisations (`init`, `instance_init`, `reset_controls`, `clear`), the arities and the sample rate; why it is `Send` and `Sync` |
| `src/controls.rs` | `Control`, `ControlKind`; privately, the `UIGlue` walk that finds the controls and builds their `MapUI` paths, and the `MetaGlue` sink of `Dsp::metadata` |
| `src/backend.rs` | private: `RawFactory` and `RawInstance`, the one place that calls the C entry points of `interp-ffi` and `cranelift-ffi`, and so the crate's `unsafe` |
| `tests/api.rs`, `tests/ddsp.rs`, `tests/allocation.rs` | the contract on both backends: lifecycle, controls, precision, threads; DDSP programs through the API; no allocation in `compute` |

Its documentation is rustdoc: `cargo doc -p faust --open` renders it, the
crate page first. It builds without warning, and the crate page describes the
model, but not every public item has a doc comment yet: the plain accessors
(`Dsp::backend`, `num_inputs`, `sample_rate`, `control`, ...), the fields of
`Control`, `CompileOptions` and `Error`, and the variants of `ControlKind` and
`Precision` have none, and the functions that return a `Result` have no
`# Errors` section. The lints list what is missing:

```bash
cargo clippy -p faust --lib -- -W missing_docs -W clippy::missing_errors_doc
```

## Which layer to bind

| The host or binding is written in | Bind | Through |
| --- | --- | --- |
| C or C++ | layer 2 | `libfaust-rs` and its headers |
| Python with Cython, cffi or ctypes, or any language with a C FFI | layer 2 | `libfaust-rs`, as `cyfaust` binds the C++ libfaust |
| JavaScript, with `faustwasm` | layer 2 | `wasm-ffi` |
| Rust | layer 3 | the `faust` crate |
| Rust, exposing faust-rs to another language (PyO3, napi-rs, ...) | layer 3 | the `faust` crate |

A binding written in Rust has both on offer, and layer 3 is the one to take.
Binding layer 2 from Rust would cost:

- **`unsafe` code in the binding.** The raw factory and instance pointers,
  their lifetimes and the callback tables are handled once, in `faust`.
- **Rebuilding the user-interface walk.** Listing the controls of a program
  means answering the `UIGlue` callbacks and building the `MapUI`-style paths
  from the group labels; `faust` does it (`Dsp::controls`, `Dsp::set`,
  `Dsp::get`).
- **`f32` samples for the interpreter.** As in C++, the C entry point
  `computeCInterpreterDSPInstance` exchanges `FAUSTFLOAT**`, `float**` in
  `libfaust-rs`, so a `-double` interpreter program has its input and output
  rounded to `f32`. The `f64` path, `interp_ffi::instance::compute_f64`, is a
  Rust function, not part of the C API; `faust` uses it, so
  `Dsp::compute_f64` is exact on both backends. (The Cranelift C entry point
  runs at the compiled precision behind the same `float**` signature.)

A binding not written in Rust has no layer 3 to reach: layer 2 is the one for
it, and it is the contract `libfaust-rs` keeps with C and C++ hosts.

## A Rust client

The crate is not on crates.io: a host depends on it by path, or on the
repository through a `git` dependency.

```toml
[dependencies]
faust = { path = "../faust-rs/crates/faust" }
```

A complete program: it compiles a one-pole smoother for the Cranelift JIT in
double precision, lists its controls, sets one by its path, runs a block, then
moves the instance to another thread, after the host has dropped its factory
handle.

```rust
use faust::{Backend, CompileOptions, ErrorKind, Factory, Precision};

/// A one-pole smoother: its pole is a slider.
const SOURCE: &str = r#"
process = _ * (1 - p) : + ~ *(p)
with { p = hslider("pole [style:knob]", 0.9, 0, 0.999, 0.001); };
"#;

fn main() -> Result<(), faust::Error> {
    // Compile once, for the Cranelift JIT, in double precision (`-double`).
    let options = CompileOptions {
        backend: Backend::Cranelift,
        precision: Precision::F64,
        ..CompileOptions::default()
    };
    let factory = Factory::from_source("smoother", SOURCE, &options)?;
    let mut dsp = factory.instantiate(48_000)?;

    // The controls, addressed by their `MapUI`/OSC path.
    for control in dsp.controls() {
        println!(
            "{} {:?} in [{}, {}], now {}",
            control.path, control.kind, control.min, control.max, dsp.get(&control.path)?
        );
    }
    let pole = dsp.control("/smoother/pole").expect("declared by the program");
    let value = pole.clamp(0.99); // `set` writes a value as given
    dsp.set("/smoother/pole", value)?;

    // A misspelled path is an error, never a silent no-op.
    let error = dsp.set("/smoother/pol", 0.5).unwrap_err();
    assert_eq!(error.kind, ErrorKind::UnknownControl);

    // One block of the impulse response, in `f64` since the program is `-double`.
    let mut input = vec![0.0_f64; 64];
    input[0] = 1.0;
    let mut output = vec![0.0_f64; 64];
    dsp.compute_f64(&[&input], &mut [&mut output])?;
    println!("impulse response: {:?}", &output[..3]);

    // A `Dsp` is `Send` and keeps its program alive: the host can drop its
    // factory handle and move the instance to an audio thread. Buffers of the
    // other width are converted.
    drop(factory);
    let audio = std::thread::spawn(move || -> Result<f32, faust::Error> {
        let silence = [0.0_f32; 64];
        let mut out = [0.0_f32; 64];
        dsp.compute_f32(&[&silence], &mut [&mut out])?;
        Ok(out[0])
    });
    let next = audio.join().expect("the audio thread")?;
    println!("next block starts at {next}");
    Ok(())
}
```

It prints:

```text
/smoother/pole HorizontalSlider in [0, 0.999], now 0.9
impulse response: [0.010000000000000009, 0.00990000000000001, 0.00980100000000001]
next block starts at 0.005255965
```

`Backend::Interp` in place of `Backend::Cranelift` runs the same program on
the interpreter, with the same results; `Factory::from_file` compiles a
`.dsp` file, and `CompileOptions::import_dirs` and `args` carry `-I` and the
other compiler options.

## See also

- [README: use `libfaust-rs` from C and C++](../README.md#use-libfaust-rs-from-c-and-c),
  the C API and the Rust API with examples.
- The `faust` crate documentation: `cargo doc -p faust --open`.
- [`crates/faust-ffi`](../crates/faust-ffi/README.md), the `libfaust-rs`
  build; [`crates/wasm-ffi`](../crates/wasm-ffi/README.md), the `faustwasm`
  compiler module.
