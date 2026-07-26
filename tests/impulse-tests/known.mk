#
# Per-DSP tolerance overrides and known-failure lists.
#
# Included by common.mk, so every backend makefile sees these. Two mechanisms:
#
#   PRECISION_<dsp>     filesCompare tolerance override for <dsp> (all backends).
#                       Used only for genuine *bounded* rounding bands; each
#                       entry records the observed max |delta| and where.
#
#   KNOWN_FAIL_all      DSPs excluded from every backend's default pass/fail gate.
#   KNOWN_FAIL_<backend> DSPs excluded for one backend (outdir name: cpp/c/interp).
#                       These are real divergences/gaps to fix, not rounding.
#
# Excluded cases are simply not built by the aggregate targets; build one
# explicitly (e.g. `make ir/interp/sound.ir`) to see it fail.
#
# Baseline measured 2026-06-14 (93 DSPs, -double).

# --- bounded tolerance bands (max |delta| just above the 2e-6 default) -------
PRECISION_mixer            := 1e-5   # 6e-6  smoothed-gain init, pass 1
PRECISION_cubic_distortion := 1e-4   # 1.4e-5 pass 1
PRECISION_gate_compressor  := 1e-3   # 2e-4  pass 1
PRECISION_vcf_wah_pedals   := 1e-3   # 1.45e-4 pass 1
PRECISION_harpe            := 1e-5   # 2e-6  polyphonic pass (c backend)
PRECISION_noise            := 1e-5   # 2e-6  polyphonic pass (c backend)
PRECISION_noiseabs         := 1e-5   # 3e-6  polyphonic pass (c backend)
PRECISION_comb_bug_exp     := 1e-3   # 1.1e-4 polyphonic pass (c backend)

# --- shared compile gap ------------------------------------------------------
KNOWN_FAIL_all := subcontainer1      # faust-rs sub-container codegen gap (compile-fail)

# --- C++ backend: full parity otherwise --------------------------------------
KNOWN_FAIL_cpp :=

# --- C backend ---------------------------------------------------------------
# grain3 was fixed by preserving full double literal precision in the C emitter.
KNOWN_FAIL_c :=

# --- Cranelift JIT backend (64-bit) ------------------------------------------
# Runs in `-double`.
# (bells/karplus/karplus32, UITester, reverb_designer/reverb_tester, sound,
#  and grain3 were fixed by matching the C++ impulse UI/soundfile harness and
#  coercing mixed-type select2 branches before CLIF emission.)
# (prefix, phasor were fixed by running the JIT instanceClear at init.)
# (table2 was fixed by following the Faust C++ lifecycle contract: compiled
#  instanceConstants initializes rwtable storage; compiled instanceClear does
#  not zero it unless the FIR clear body says so.)
KNOWN_FAIL_cranelift :=

# --- interpreter backend -----------------------------------------------------
# The former UI/soundfile gaps were fixed by matching the C++ impulse harness:
# `FUI::setButtons` only drives buttons, and soundfile tests use the same
# `TestMemoryReader` fixture.
# (comb_delay1/2, math_simp, norm3 were fixed by honoring is_reverse in the
#  general ForLoop compiler -- the shift-array delay strategy now runs.)
KNOWN_FAIL_interp :=

# --- WASM backend (64-bit scalar prefix through Node WebAssembly) ------------
# Matches the scalar prefix once the Node runner mirrors the C++ impulse
# harness: all input channels receive the first-frame impulse and soundfile
# widgets use the TestMemoryReader fixture in WASM linear memory.
KNOWN_FAIL_wasm :=

# --- AssemblyScript backend (scalar prefix through asc + Node) ---------------
# Matches the scalar prefix in `-double`: the Node runner forwards precision,
# mirrors the C++ impulse harness controls, and installs the same soundfile
# fixture through imported soundfile helpers.
KNOWN_FAIL_assemblyscript :=

# --- Rust backend (scalar prefix through rustc) ------------------------------
# Generated source is appended to archs/impulserust.rs and compiled natively.
KNOWN_FAIL_rust :=

# --- Julia backend (scalar prefix through the local Julia harness) ----------
KNOWN_FAIL_julia :=

# --- mode/scheduling variants ------------------------------------------------
# Variant outdirs inherit their base backend's known failures. Any divergence
# specific to one mode/strategy can be added as KNOWN_FAIL_<outdir>, for example
# KNOWN_FAIL_cpp-vec0-ss2.
KNOWN_FAIL_cpp-vec0 :=
KNOWN_FAIL_cpp-vec1 :=
KNOWN_FAIL_c-vec0 :=
KNOWN_FAIL_c-vec1 :=
KNOWN_FAIL_interp-vec0 :=
KNOWN_FAIL_interp-vec1 :=
KNOWN_FAIL_cranelift-vec0 :=
KNOWN_FAIL_cranelift-vec1 :=
KNOWN_FAIL_wasm-vec0 :=
KNOWN_FAIL_wasm-vec1 :=
KNOWN_FAIL_assemblyscript-vec0 :=
KNOWN_FAIL_assemblyscript-vec1 :=
KNOWN_FAIL_rust-vec0 :=
KNOWN_FAIL_rust-vec1 :=
KNOWN_FAIL_julia-vec0 :=
KNOWN_FAIL_julia-vec1 :=

# Execution-option variants (`-ec`, `-os`, both). They inherit their base
# backend's list through `base_backend`; entries here are divergences specific to
# an execution-option shape.
#
# `bs` is not a divergence but a deliberate compiler rejection: it reads the
# foreign variable `count`, which the execution options forbid
# (`FRS-SFIR-0009`, "accessing foreign variable 'count' is not allowed in this
# compilation mode"). Excluded from the gate rather than silenced, so the
# rejection stays observable with `make ir/cpp-ec/bs.ir`.
KNOWN_FAIL_cpp-ec := bs
KNOWN_FAIL_cpp-os := bs
KNOWN_FAIL_cpp-ec-os := bs
KNOWN_FAIL_c-ec := bs
KNOWN_FAIL_c-os := bs
KNOWN_FAIL_c-ec-os := bs
KNOWN_FAIL_rust-ec := bs
KNOWN_FAIL_rust-os := bs
KNOWN_FAIL_rust-ec-os := bs

# Tolerance to apply when a per-DSP override exists, else the global `precision`.
dsp_precision = $(if $(PRECISION_$1),$(PRECISION_$1),$(precision))
# Strip the scheduling suffix before the vector suffix:
# `cpp-vec0-ss2` -> `cpp-vec0` -> `cpp`.
without_ss = $(patsubst %-ss0,%,$(patsubst %-ss1,%,$(patsubst %-ss2,%,$(patsubst %-ss3,%,$1))))
# Execution-option suffixes: `cpp-ec-os` -> `cpp`, `cpp-ec` -> `cpp`,
# `cpp-os` -> `cpp`. `make` expands the innermost `patsubst` first, so `-os` is
# stripped before `-ec`; `cpp-ec-os` therefore passes through `cpp-ec` and lands
# on `cpp` either way. Verified for every outdir in use, including the existing
# `-vecN` / `-ssN` variants, which these rules must leave untouched.
without_exec = $(patsubst %-ec-os,%,$(patsubst %-ec,%,$(patsubst %-os,%,$1)))
base_backend = $(patsubst %-vec0,%,$(patsubst %-vec1,%,$(call without_ss,$(call without_exec,$1))))
# Names excluded for a given backend outdir. Every variant inherits its base
# backend's known failures plus any exact outdir-specific list.
known_fail_for = $(KNOWN_FAIL_all) $(KNOWN_FAIL_$(call base_backend,$1)) $(KNOWN_FAIL_$1)
