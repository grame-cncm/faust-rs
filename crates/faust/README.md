# faust

The Rust API of faust-rs: compile a Faust program to a `Factory`, instantiate
it as `Dsp` values, set their parameters and run them.

This crate and the C API of `libfaust-rs` (`faust-ffi`) are the two supported
contracts. The other workspace crates (`compiler`, `codegen`, `interp-ffi`,
`cranelift-ffi`, ...) are implementation details with no stability promise.

## Model

- `CompileOptions::backend` selects the bytecode interpreter
  (`Backend::Interp`) or the Cranelift JIT (`Backend::Cranelift`).
- A `Factory` is a compiled program (arities, JSON description, code).
  `Factory::from_source` / `Factory::from_file` create it; clones are handles
  on the same program.
- A `Dsp` is an instance created by `Factory::create_dsp_instance`. It keeps
  its factory alive, needs no lifetime parameter, and is `Send + Sync`.
- Parameters are addressed as the C++ `MapUI` does, by path, shortname or
  label (`Dsp::set_param_value`, `Dsp::get_param_value`); `Dsp::params` lists
  them.
- `Dsp::compute` takes `f32` or `f64` host buffers and converts when they
  differ from the compiled precision (`-double` / `-single` in
  `CompileOptions::args`; single by default).

Methods with a C++ `dsp` / `dsp_factory` / `MapUI` counterpart carry its name
in snake case (`get_num_inputs`, `instance_init`, `get_json`, ...). Both
backends are driven through their C entry points, so factories are shared by
SHA key and reference counted exactly as in the C API.

The crate-level documentation (`src/lib.rs`) has the full name mapping, the
precision rules, the known metadata gap and an example.

## Relationship to other crates

- `interp-ffi`, `cranelift-ffi` — the two backends, driven through their C
  entry points; this crate owns their raw factory and instance pointers and
  exposes no `unsafe` API.
- `ffi-common` — UI/metadata callback tables and `ControlRange`.
- `codegen` — shortname computation for the parameter paths
  (`codegen::shortname`).

See also DIFF-API-008 in `porting/faust-rs-vs-faust-cpp-differences-en.md`.
