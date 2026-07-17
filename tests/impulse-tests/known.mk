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
# Generated source is appended to archs/impulserust.rs (host Meta/UI traits +
# the shared impulse/soundfile fixture) and compiled natively with rustc.
KNOWN_FAIL_rust :=

# Tolerance to apply when a per-DSP override exists, else the global `precision`.
dsp_precision = $(if $(PRECISION_$1),$(PRECISION_$1),$(precision))
# Names excluded for a given backend outdir.
known_fail_for = $(KNOWN_FAIL_all) $(KNOWN_FAIL_$1)
