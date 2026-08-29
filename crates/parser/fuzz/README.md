# Parser fuzzing

Fuzzes `parser::parse_program` with libFuzzer via [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz):
any input, valid or not, must return a `ParseOutput` (possibly with errors)
without panicking or hanging.

Requirements: `cargo install cargo-fuzz` and a nightly toolchain.

Run from `crates/parser`:

```bash
cargo +nightly fuzz run parse_program
```

Useful variants:

```bash
# time-bounded session
cargo +nightly fuzz run parse_program -- -max_total_time=300

# reproduce a crash found earlier
cargo +nightly fuzz run parse_program artifacts/parse_program/<crash-file>
```

`corpus/parse_program/` holds seed inputs (checked in); new corpus entries
found while fuzzing accumulate there. Crashing inputs land in
`artifacts/parse_program/` (gitignored).
