# Known failures and tolerance overrides

Baseline characterized 2026-06-14 (93 DSPs, `-double`, default tolerance
`2e-06`). The machinery for these lives in [`known.mk`](known.mk):

- `PRECISION_<dsp>` — a looser `filesCompare` tolerance for a genuinely bounded
  rounding band (applied to every backend; the C/C++ backends already pass these
  at delta 0, so it only affects the cases that need it).
- `KNOWN_FAIL_<backend>` / `KNOWN_FAIL_all` — cases excluded from a backend's
  default pass/fail gate. They are not built by the aggregate targets; build one
  explicitly (`make ir/interp/sound.ir`) to reproduce the failure.

When a divergence is fixed, remove the entry so the gate re-covers it.

## Shared compile gap

| DSP | Backends | Cause |
|---|---|---|
| `subcontainer1` | all | faust-rs sub-container codegen gap (does not compile) |

## Tolerance overrides (bounded rounding)

| DSP | Tol | Max \|Δ\| | Where | Note |
|---|---|---|---|---|
| `mixer` | 1e-5 | 6e-6 | pass 1 | smoothed-gain init |
| `cubic_distortion` | 1e-4 | 1.4e-5 | pass 1 | |
| `gate_compressor` | 1e-3 | 2e-4 | pass 1 | |
| `vcf_wah_pedals` | 1e-3 | 1.45e-4 | pass 1 | |
| `harpe` | 1e-5 | 2e-6 | poly pass | C backend |
| `noise` | 1e-5 | 2e-6 | poly pass | C backend (not an LCG bug) |
| `noiseabs` | 1e-5 | 3e-6 | poly pass | C backend |
| `comb_bug_exp` | 1e-3 | 1.1e-4 | poly pass | C backend |

## C / C++ / interpreter / Cranelift / WASM / AssemblyScript / Rust exclusions

No backend-specific exclusions remain for C, C++, interpreter, Cranelift, WASM,
AssemblyScript, or Rust. Only the shared `subcontainer1` compile gap is
excluded for those gates.
