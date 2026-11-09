#
# faust-rs impulse-response test suite — top-level driver.
#
# Mirrors the C++ Faust `tests/impulse-tests` machinery: build reference impulse
# responses with the genuine C++ Faust compiler, then check each faust-rs
# backend against them with `filesCompare`.
#
# Typical workflow:
#   make build          # cargo build the faust-rs compiler + impulse runner
#   make reference      # generate the C++ reference .ir files (the oracle)
#   make interp         # check the faust-rs interpreter backend
#   make cpp            # check the faust-rs C++ backend
#   make c              # check the faust-rs C backend
#   make wasm           # check the faust-rs WASM backend
#   make assemblyscript # check the faust-rs AssemblyScript backend
#   make cmajor         # check scalar Cmajor through cmaj-generated C++
#   make bench          # compare C++ Faust vs faust-rs generated-code speed
#   make compile-bench  # compare C++ Faust vs faust-rs compile time
#   make all            # interp + c + cpp + cranelift
#
# Add `-k` to keep going past failing cases and `-j N` for parallelism, e.g.
#   make -k -j8 cpp
#
# There is no destructive clean of references: `make clean` only removes the
# generated `ir/` and `build/` trees. Use `make distclean` to also drop
# `reference/`.
#

include common.mk

MAKE ?= make

BACKENDS := cpp c interp cranelift wasm assemblyscript rust julia
SS_VALUES := 0 1 2 3
SCALAR_SS_TARGETS := $(foreach backend,$(BACKENDS),$(foreach ss,$(SS_VALUES),$(backend)-ss$(ss)))
VECTOR_SS_TARGETS := $(foreach backend,$(BACKENDS),$(foreach lv,0 1,$(foreach ss,$(SS_VALUES),$(backend)-vec$(lv)-ss$(ss))))
BACKEND_MATRIX_SMOKE_DSPFILES ?= dsp/APF.dsp dsp/delays.dsp dsp/select2.dsp

.PHONY: all all-vec0 all-vec1 all-vec all-ss all-vec-ss backend-matrix backend-matrix-smoke backend-matrix-clean backend-matrix-report backend-matrix-full reference reference-report reference-libs-accept interp interp-vec0 interp-vec1 c c-vec0 c-vec1 cpp cpp-vec0 cpp-vec1 cranelift cranelift-vec0 cranelift-vec1 wasm wasm-vec0 wasm-vec1 assemblyscript assemblyscript-vec0 assemblyscript-vec1 rust rust-vec0 rust-vec1 julia julia-vec0 julia-vec1 mojo mojo-vec0 mojo-vec1 mojo-outline mojo-gpu mojo-gpu-batch mojo-gpu-poly mojo-gpu-bells mojo-gpu-choir mojo-gpu-choir-build mojo-gpu-choir-reverb mojo-gpu-choir-render mojo-gpu-modal-fad mojo-gpu-modal-fad-build mojo-gpu-modal-fad-bell mojo-gpu-bench mojo-gpu-bench-batch cmajor cpp-ec cpp-os cpp-ec-os c-ec c-os c-ec-os rust-ec rust-os rust-ec-os all-execopts cpp-mem0 c-mem0 cranelift-mem0 all-mem0 cpp-mem0-smoke c-mem0-smoke cranelift-mem0-smoke mem0-smoke mem0-opt mem0-sanitize bench bench-self-test vec-bench compile-bench build tools help clean distclean

all: cpp c interp cranelift wasm assemblyscript rust julia

all-vec0: cpp-vec0 c-vec0 interp-vec0 cranelift-vec0 wasm-vec0 assemblyscript-vec0 rust-vec0 julia-vec0

all-vec1: cpp-vec1 c-vec1 interp-vec1 cranelift-vec1 wasm-vec1 assemblyscript-vec1 rust-vec1 julia-vec1

all-vec: all-vec0 all-vec1

# Execution options (`-ec`, `-os`, and both) on every backend whose capability
# table marks them Explicit and that has a runnable impulse target: cpp, c and
# rust. interp/cranelift/wasm/wast/julia are Unsupported, and `fir` is a text
# dump with no runtime, so there is nothing to run there.
#
# Each shape is compared against the SAME classic `reference/*.ir` as every
# other target: switching execution options must not move the impulse response
# by one bit.
all-execopts: cpp-ec cpp-os cpp-ec-os c-ec c-os c-ec-os rust-ec rust-os rust-ec-os

cpp-mem0:
	$(MAKE) -f Make.mem0 cpp

c-mem0:
	$(MAKE) -f Make.mem0 c

cranelift-mem0:
	$(MAKE) -f Make.mem0 cranelift

all-mem0:
	$(MAKE) -f Make.mem0 all
	$(MAKE) -f Make.mem0 audit $(MEM0_SMOKE_ARGS)

MEM0_SMOKE_ARGS := dspdir=dsp-mem0 mem0_frames=512 mem0_local_reference=1 \
	MEM0_ROOT=build/mem0-smoke MEM0_IR=ir/mem0-smoke

cpp-mem0-smoke:
	$(MAKE) -f Make.mem0 cpp $(MEM0_SMOKE_ARGS)

c-mem0-smoke:
	$(MAKE) -f Make.mem0 c $(MEM0_SMOKE_ARGS)

cranelift-mem0-smoke:
	$(MAKE) -f Make.mem0 cranelift $(MEM0_SMOKE_ARGS)

mem0-smoke:
	$(MAKE) -f Make.mem0 audit $(MEM0_SMOKE_ARGS)

mem0-opt:
	$(MAKE) -f Make.mem0 opt $(MEM0_SMOKE_ARGS)

mem0-sanitize:
	$(MAKE) -f Make.mem0 sanitize $(MEM0_SMOKE_ARGS)

all-ss: $(SCALAR_SS_TARGETS)

all-vec-ss: $(VECTOR_SS_TARGETS)

backend-matrix: all-ss all-vec-ss

backend-matrix-smoke:
	$(MAKE) backend-matrix dspfiles="$(BACKEND_MATRIX_SMOKE_DSPFILES)"

backend-matrix-clean:
	rm -rf $(addprefix ir/,$(SCALAR_SS_TARGETS) $(VECTOR_SS_TARGETS))
	rm -rf $(addprefix build/,$(SCALAR_SS_TARGETS) $(VECTOR_SS_TARGETS))

backend-matrix-report:
	cd ../.. && cargo run -p xtask -- p7-matrix-report

backend-matrix-full:
	$(MAKE) backend-matrix-clean
	$(MAKE) backend-matrix
	$(MAKE) backend-matrix-report

# Each matrix outdir is unique so combinations can run in
# parallel without sharing generated artifacts.
define DEFINE_SCHEDULE_TARGETS
$(1)-ss%: reference
	$$(MAKE) -f $(2) all outdir=$$@ $(3) COMPILER_OPTS="-ss $$*"

$(1)-vec0-ss%: reference
	$$(MAKE) -f $(2) all outdir=$$@ $(3) COMPILER_OPTS="-vec -lv 0 -ss $$*"

$(1)-vec1-ss%: reference
	$$(MAKE) -f $(2) all outdir=$$@ $(3) COMPILER_OPTS="-vec -lv 1 -ss $$*"
endef

$(eval $(call DEFINE_SCHEDULE_TARGETS,cpp,Make.gcc,lang=cpp))
$(eval $(call DEFINE_SCHEDULE_TARGETS,c,Make.gcc,lang=c))
$(eval $(call DEFINE_SCHEDULE_TARGETS,interp,Make.interp,))
$(eval $(call DEFINE_SCHEDULE_TARGETS,cranelift,Make.cranelift,))
$(eval $(call DEFINE_SCHEDULE_TARGETS,wasm,Make.wasm,))
$(eval $(call DEFINE_SCHEDULE_TARGETS,assemblyscript,Make.assemblyscript,))
$(eval $(call DEFINE_SCHEDULE_TARGETS,rust,Make.rust,))
$(eval $(call DEFINE_SCHEDULE_TARGETS,julia,Make.julia,))

reference:
	$(MAKE) -f Make.ref reference

reference-report:
	$(MAKE) -f Make.ref reference-report

reference-libs-accept:
	$(MAKE) -f Make.ref reference-libs-accept

# Every ordinary differential target first refreshes the C++ oracle manifest.
# This lets a new C++ compiler rejection exclude only the affected DSPs before
# any faust-rs backend asks for a missing `reference/<dsp>.ir` file.
CPP_ORACLE_CHECK_TARGETS := all all-vec0 all-vec1 all-vec all-ss all-vec-ss backend-matrix \
	interp interp-vec0 interp-vec1 c c-vec0 c-vec1 cpp cpp-vec0 cpp-vec1 \
	cranelift cranelift-vec0 cranelift-vec1 wasm wasm-vec0 wasm-vec1 \
	assemblyscript assemblyscript-vec0 assemblyscript-vec1 rust rust-vec0 rust-vec1 \
	julia julia-vec0 julia-vec1 cmajor cpp-ec cpp-os cpp-ec-os c-ec c-os c-ec-os \
	rust-ec rust-os rust-ec-os all-execopts cpp-mem0 c-mem0 cranelift-mem0 all-mem0
$(CPP_ORACLE_CHECK_TARGETS): reference

# ── RAD lane ─────────────────────────────────────────────────────────────────
#
# `rad` and `fad` are faust-rs primitives; C++ Faust rejects them with
# `undefined symbol : rad`, so these programs have no C++ oracle. The lane
# takes its reference from the faust-rs interpreter (`Make.ref-rad`, which
# documents why that is sound) and then checks that every other backend
# computes the same gradients.
#
# `rad-reference` must be built first, like `reference` for the ordinary suite.

RAD_DIRS := dspdir=dsp-rad refdir=reference-rad

# The C++ architecture emits all four reference passes while the reference
# covers the scalar one only, so the shorter file goes first — see the
# COMPARE_ARGS comment in Make.gcc.
RAD_COMPARE_PREFIX := COMPARE_ARGS='$$(refdir)/$$*.ir $$@ -part $$(call dsp_precision,$$*)'

.PHONY: rad-reference rad-interp rad-cpp rad-c rad-rust rad-julia rad-wasm \
        rad-assemblyscript rad-cranelift all-rad

rad-reference:
	$(MAKE) -f Make.ref-rad reference

rad-interp:
	$(MAKE) -f Make.interp all outdir=rad-interp $(RAD_DIRS)

rad-cpp:
	$(MAKE) -f Make.gcc all outdir=rad-cpp lang=cpp $(RAD_DIRS) $(RAD_COMPARE_PREFIX)

rad-c:
	$(MAKE) -f Make.gcc all outdir=rad-c lang=c $(RAD_DIRS) $(RAD_COMPARE_PREFIX)

rad-rust:
	$(MAKE) -f Make.rust all outdir=rad-rust $(RAD_DIRS)

rad-julia:
	$(MAKE) -f Make.julia all outdir=rad-julia $(RAD_DIRS)

rad-wasm:
	$(MAKE) -f Make.wasm all outdir=rad-wasm $(RAD_DIRS)

rad-assemblyscript:
	$(MAKE) -f Make.assemblyscript all outdir=rad-assemblyscript $(RAD_DIRS)

rad-cranelift:
	$(MAKE) -f Make.cranelift all outdir=rad-cranelift $(RAD_DIRS)

# Every backend that emits RAD code. `rad-interp` is deliberately absent: it
# would compare the reference against itself and prove nothing.
all-rad: rad-cpp rad-c rad-rust rad-julia rad-wasm rad-assemblyscript rad-cranelift

interp:
	$(MAKE) -f Make.interp all outdir=interp

interp-vec0:
	$(MAKE) -f Make.interp all outdir=interp-vec0 VECOPTS="-vec -lv 0"

interp-vec1:
	$(MAKE) -f Make.interp all outdir=interp-vec1 VECOPTS="-vec -lv 1"

cpp:
	$(MAKE) -f Make.gcc all outdir=cpp lang=cpp

cpp-vec0:
	$(MAKE) -f Make.gcc all outdir=cpp-vec0 lang=cpp VECOPTS="-vec -lv 0"

cpp-vec1:
	$(MAKE) -f Make.gcc all outdir=cpp-vec1 lang=cpp VECOPTS="-vec -lv 1"

cpp-ec:
	$(MAKE) -f Make.execopts all outdir=cpp-ec lang=cpp EXECOPTS="-ec" EXECDEFS="-DIMPULSE_EC"

cpp-os:
	$(MAKE) -f Make.execopts all outdir=cpp-os lang=cpp EXECOPTS="-os" EXECDEFS="-DIMPULSE_OS"

cpp-ec-os:
	$(MAKE) -f Make.execopts all outdir=cpp-ec-os lang=cpp EXECOPTS="-ec -os" EXECDEFS="-DIMPULSE_EC -DIMPULSE_OS"

c:
	$(MAKE) -f Make.gcc all outdir=c lang=c

c-vec0:
	$(MAKE) -f Make.gcc all outdir=c-vec0 lang=c VECOPTS="-vec -lv 0"

c-vec1:
	$(MAKE) -f Make.gcc all outdir=c-vec1 lang=c VECOPTS="-vec -lv 1"

c-ec:
	$(MAKE) -f Make.execopts all outdir=c-ec lang=c EXECOPTS="-ec" EXECDEFS="-DIMPULSE_EC"

c-os:
	$(MAKE) -f Make.execopts all outdir=c-os lang=c EXECOPTS="-os" EXECDEFS="-DIMPULSE_OS"

c-ec-os:
	$(MAKE) -f Make.execopts all outdir=c-ec-os lang=c EXECOPTS="-ec -os" EXECDEFS="-DIMPULSE_EC -DIMPULSE_OS"

cranelift:
	$(MAKE) -f Make.cranelift all outdir=cranelift

cranelift-vec0:
	$(MAKE) -f Make.cranelift all outdir=cranelift-vec0 VECOPTS="-vec -lv 0"

cranelift-vec1:
	$(MAKE) -f Make.cranelift all outdir=cranelift-vec1 VECOPTS="-vec -lv 1"

wasm:
	$(MAKE) -f Make.wasm all outdir=wasm

wasm-vec0:
	$(MAKE) -f Make.wasm all outdir=wasm-vec0 VECOPTS="-vec -lv 0"

wasm-vec1:
	$(MAKE) -f Make.wasm all outdir=wasm-vec1 VECOPTS="-vec -lv 1"

rust:
	$(MAKE) -f Make.rust all outdir=rust

rust-vec0:
	$(MAKE) -f Make.rust all outdir=rust-vec0 VECOPTS="-vec -lv 0"

rust-vec1:
	$(MAKE) -f Make.rust all outdir=rust-vec1 VECOPTS="-vec -lv 1"

rust-ec:
	$(MAKE) -f Make.rust all outdir=rust-ec EXECOPTS="-ec" EXECCFGS="--cfg impulse_ec"

rust-os:
	$(MAKE) -f Make.rust all outdir=rust-os EXECOPTS="-os" EXECCFGS="--cfg impulse_os"

rust-ec-os:
	$(MAKE) -f Make.rust all outdir=rust-ec-os EXECOPTS="-ec -os" EXECCFGS="--cfg impulse_ec --cfg impulse_os"

julia:
	$(MAKE) -f Make.julia all outdir=julia

julia-vec0:
	$(MAKE) -f Make.julia all outdir=julia-vec0 VECOPTS="-vec -lv 0"

julia-vec1:
	$(MAKE) -f Make.julia all outdir=julia-vec1 VECOPTS="-vec -lv 1"

# Mojo is an opt-in external lane: it needs the Mojo toolchain (see Make.mojo),
# so it is absent from `all` and from the scheduling matrix, as Cmajor is.
mojo:
	$(MAKE) -f Make.mojo all outdir=mojo

mojo-vec0:
	$(MAKE) -f Make.mojo all outdir=mojo-vec0 VECOPTS="-vec -lv 0"

mojo-vec1:
	$(MAKE) -f Make.mojo all outdir=mojo-vec1 VECOPTS="-vec -lv 1"

# Every `then`/`else` block of every program outlined into a method of its
# own (`fir::outline` with budget and minimum 1, plan
# porting/mojo-large-program-outlining-plan-2026-11-09-en.md): the scalar
# contract through the compiler's `mojo_batch --scalar`, checked against the
# C++ reference as `make mojo` is.
mojo-outline:
	cargo build --release -p compiler --example mojo_batch
	$(MAKE) -f Make.mojo all outdir=mojo-outline \
	  MOJO_BATCH="../../target/release/examples/mojo_batch --scalar --double --outline-budget 1 --outline-min-block 1"

# Many voices of each program on the GPU, one thread per voice (plan
# porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md, G1). The device has
# no f64: `-single`.
mojo-gpu:
	$(MAKE) -f Make.mojo all outdir=mojo-gpu MOJO_PRECISION=-single MOJO_REF_COMPARE=0 \
	  ARCH_MOJO_TAIL=archs/runimpulsemojogpu.mojo \
	  MOJOBUILDOPTIONS="-O3 -D DFAUST=DType.float32 $(MOJOGPUOPTIONS)"

# The batch contract on the GPU (G3.5): one struct for N voices, state as a
# structure of arrays, emitted by the compiler's `mojo_batch` example; the
# checks of `mojo-gpu`.
mojo-gpu-batch:
	cargo build --release -p compiler --example mojo_batch
	$(MAKE) -f Make.mojo all outdir=mojo-gpu-batch MOJO_PRECISION=-single MOJO_REF_COMPARE=0 \
	  MOJO_BATCH=../../target/release/examples/mojo_batch \
	  ARCH_MOJO_TAIL=archs/runimpulsemojogpubatch.mojo \
	  MOJOBUILDOPTIONS="-O3 -D DFAUST=DType.float32 $(MOJOGPUOPTIONS)"

# A polyphonic instrument played in real time (G4): POLY_VOICES voices of
# POLY_DSP (../corpus/gpu_voice_saw.dsp) on the GPU with the batch contract, mixed on the device and
# written through PortAudio (muted unless POLY_AUDIBLE=audible); fails on an
# underrun. PORTAUDIO_LIB names the directory of libportaudio.
MOJO ?= mojo
POLY_VOICES ?= 256
POLY_SECONDS ?= 10
POLY_BLOCK ?= 256
POLY_AUDIBLE ?=
POLY_DSP ?= ../corpus/gpu_voice_saw.dsp
PORTAUDIO_LIB ?= /opt/homebrew/lib
mojo-gpu-poly:
	cargo build --release -p compiler --example mojo_batch
	mkdir -p build/mojo-gpu-poly
	../../target/release/examples/mojo_batch $(POLY_DSP) -o build/mojo-gpu-poly/voice.mojo
	cat archs/impulsemojo.mojo build/mojo-gpu-poly/voice.mojo archs/polymojogpubatch.mojo > build/mojo-gpu-poly/poly_main.mojo
	$(MOJO) build build/mojo-gpu-poly/poly_main.mojo -o build/mojo-gpu-poly/poly -O3 -D DFAUST=DType.float32 \
	  -Xlinker -L$(PORTAUDIO_LIB) -Xlinker -lportaudio -Xlinker -rpath -Xlinker $(PORTAUDIO_LIB)
	build/mojo-gpu-poly/poly $(POLY_VOICES) $(POLY_SECONDS) $(POLY_AUDIBLE) $(POLY_BLOCK)

# A carillon on the GPU (modal synthesis use case): BELLS bells, each a
# `pm.strikeModel` strike and the 50 `pm.modeFilter` modes of
# `pm.standardBellModel`, one GPU instance per mode with its own frequency,
# decay and gain (../corpus/gpu_bell_strike.dsp, gpu_bell_mode.dsp; the mode
# table read from the installed physmodels.lib by tools/bell_modes.py).
# Checks one bell against the original pm.standardBell
# (../corpus/gpu_bell_reference.dsp), times BELLS bells on the GPU against
# the CPU threads, then plays them for BELLS_SECONDS (muted unless
# POLY_AUDIBLE=audible) with the score BELLS_SCORE. Needs PortAudio
# (PORTAUDIO_LIB).
BELLS ?= 256
BELLS_SECONDS ?= 10
# cloud (every bell, staggered), melody (Ode to Joy) or chime (Westminster
# Quarters); the two tunes want BELLS=37, one bell per semitone.
BELLS_SCORE ?= cloud
mojo-gpu-bells:
	cargo build --release -p compiler --example mojo_batch
	mkdir -p build/mojo-gpu-bells
	python3 tools/bell_modes.py $(FAUSTLIBS)/physmodels.lib build/mojo-gpu-bells/bell_data.mojo
	../../target/release/examples/mojo_batch ../corpus/gpu_bell_strike.dsp -I $(FAUSTLIBS) --cn strikedsp -o build/mojo-gpu-bells/strike.mojo
	../../target/release/examples/mojo_batch ../corpus/gpu_bell_mode.dsp -I $(FAUSTLIBS) --cn modedsp -o build/mojo-gpu-bells/mode.mojo
	$(FAUST_RS) -lang mojo -single -cn refdsp -I $(FAUSTLIBS) ../corpus/gpu_bell_reference.dsp -o build/mojo-gpu-bells/ref.mojo
	cat archs/impulsemojo.mojo build/mojo-gpu-bells/bell_data.mojo build/mojo-gpu-bells/strike.mojo \
	  build/mojo-gpu-bells/mode.mojo build/mojo-gpu-bells/ref.mojo archs/bellsmojogpubatch.mojo \
	  > build/mojo-gpu-bells/bells_main.mojo
	$(MOJO) build build/mojo-gpu-bells/bells_main.mojo -o build/mojo-gpu-bells/bells -O3 -D DFAUST=DType.float32 \
	  -Xlinker -L$(PORTAUDIO_LIB) -Xlinker -lportaudio -Xlinker -rpath -Xlinker $(PORTAUDIO_LIB)
	build/mojo-gpu-bells/bells validate
	build/mojo-gpu-bells/bells bench $(BELLS)
	build/mojo-gpu-bells/bells play $(BELLS) $(BELLS_SECONDS) $(POLY_AUDIBLE) $(BELLS_SCORE)

# An additive choir on the GPU (archs/choirmojogpubatch.mojo): CHOIR singers
# in four parts, every harmonic up to 20 kHz one instance of
# ../corpus/gpu_choir_partial.dsp; 448 singers are about 48,000 partials.
# Two versions: mojo-gpu-choir plays the choir dry, as the GPU computes it;
# mojo-gpu-choir-reverb plays it in a hall, its stereo mix passed through
# ../corpus/gpu_choir_reverb.dsp (re.zita_rev1_stereo) on the CPU.
CHOIR ?= 448
CHOIR_SECONDS ?= 20
mojo-gpu-choir-build:
	cargo build --release -p compiler --example mojo_batch
	mkdir -p build/mojo-gpu-choir
	../../target/release/examples/mojo_batch ../corpus/gpu_choir_partial.dsp --cn choirdsp -o build/mojo-gpu-choir/choir.mojo
	$(FAUST_RS) -lang mojo -single -cn refdsp ../corpus/gpu_choir_partial.dsp -o build/mojo-gpu-choir/ref.mojo
	$(FAUST_RS) -lang mojo -single -cn revdsp -I $(FAUSTLIBS) ../corpus/gpu_choir_reverb.dsp -o build/mojo-gpu-choir/rev.mojo
	cat archs/impulsemojo.mojo build/mojo-gpu-choir/choir.mojo build/mojo-gpu-choir/ref.mojo \
	  build/mojo-gpu-choir/rev.mojo archs/choirmojogpubatch.mojo > build/mojo-gpu-choir/choir_main.mojo
	$(MOJO) build build/mojo-gpu-choir/choir_main.mojo -o build/mojo-gpu-choir/choir -O3 -D DFAUST=DType.float32 \
	  -Xlinker -L$(PORTAUDIO_LIB) -Xlinker -lportaudio -Xlinker -rpath -Xlinker $(PORTAUDIO_LIB)
	build/mojo-gpu-choir/choir validate
mojo-gpu-choir: mojo-gpu-choir-build
	build/mojo-gpu-choir/choir bench $(CHOIR)
	build/mojo-gpu-choir/choir play $(CHOIR) $(CHOIR_SECONDS) $(POLY_AUDIBLE)
mojo-gpu-choir-reverb: mojo-gpu-choir-build
	build/mojo-gpu-choir/choir play $(CHOIR) $(CHOIR_SECONDS) $(POLY_AUDIBLE) reverb
# Both versions rendered offline to WAV files: one cycle of the dry choir
# (four chords, 20 s) and one of the choir in the hall (eight chords and
# their rests, 64 s, plus 5 s for the tail).
mojo-gpu-choir-render: mojo-gpu-choir-build
	build/mojo-gpu-choir/choir render $(CHOIR) 20 build/mojo-gpu-choir/choir-dry.f32
	python3 tools/f32_to_wav.py build/mojo-gpu-choir/choir-dry.f32 build/mojo-gpu-choir/choir-dry.wav
	build/mojo-gpu-choir/choir render $(CHOIR) 69 build/mojo-gpu-choir/choir-reverb.f32 reverb
	python3 tools/f32_to_wav.py build/mojo-gpu-choir/choir-reverb.f32 build/mojo-gpu-choir/choir-reverb.wav

# Modal sound matching by `fad` on the GPU (archs/modalfadmojogpubatch.mojo):
# a bank of MODAL modes, one instance of ../corpus/gpu_modal_fad_voice.dsp
# per mode (y and its three tangents), fitted to a synthetic target (a hidden
# bank) from a start MODAL_SIGMA bandwidths away, for MODAL_EPOCHS epochs of
# MODAL_UPDATE (gn: damped Gauss-Newton per mode; adam). Validates the
# outputs and gradients (GPU against the CPU program and the closed form),
# times one epoch on the GPU twice, with the batch code as emitted (its
# `compute` state in locals, `fir::batch::promote_state`) and with every
# state access left in the voice arenas (`mojo_batch --no-registers`), and
# against the CPU baseline: archs/modalfadcpu.cpp, the same fit with
# ../corpus/gpu_modal_fad_group.dsp (four modes per program) compiled by
# clang and run on MODAL_THREADS threads; then fits.
MODAL ?= 4096
MODAL_EPOCHS ?= 300
MODAL_SIGMA ?= 0.5
MODAL_UPDATE ?= gn
MODAL_THREADS ?= 1 4 10
CXX_MODAL ?= clang++ -std=c++17 -O3 -ffast-math -mcpu=native
mojo-gpu-modal-fad-build:
	cargo build --release -p compiler --example mojo_batch
	mkdir -p build/mojo-gpu-modal-fad
	../../target/release/examples/mojo_batch ../corpus/gpu_modal_fad_voice.dsp --cn modaldsp -o build/mojo-gpu-modal-fad/modal.mojo
	../../target/release/examples/mojo_batch ../corpus/gpu_modal_fad_voice.dsp --cn modaldsp --no-registers -o build/mojo-gpu-modal-fad/modal_arena.mojo
	$(FAUST_RS) -lang mojo -single -cn refdsp ../corpus/gpu_modal_fad_voice.dsp -o build/mojo-gpu-modal-fad/ref.mojo
	cat archs/impulsemojo.mojo build/mojo-gpu-modal-fad/modal.mojo build/mojo-gpu-modal-fad/ref.mojo \
	  archs/modalfadmojogpubatch.mojo > build/mojo-gpu-modal-fad/modal_main.mojo
	cat archs/impulsemojo.mojo build/mojo-gpu-modal-fad/modal_arena.mojo build/mojo-gpu-modal-fad/ref.mojo \
	  archs/modalfadmojogpubatch.mojo > build/mojo-gpu-modal-fad/modal_arena_main.mojo
	$(MOJO) build build/mojo-gpu-modal-fad/modal_main.mojo -o build/mojo-gpu-modal-fad/modalfad -O3 -D DFAUST=DType.float32
	$(MOJO) build build/mojo-gpu-modal-fad/modal_arena_main.mojo -o build/mojo-gpu-modal-fad/modalfad_arena -O3 -D DFAUST=DType.float32
	$(FAUST_RS) -lang cpp -single -cn plainvoice -pn plain ../corpus/gpu_modal_fad_group.dsp -o build/mojo-gpu-modal-fad/plain.cpp
	$(FAUST_RS) -lang cpp -single -cn fadvoice ../corpus/gpu_modal_fad_group.dsp -o build/mojo-gpu-modal-fad/fad.cpp
	{ cat archs/modalfadcpu.cpp build/mojo-gpu-modal-fad/plain.cpp build/mojo-gpu-modal-fad/fad.cpp; \
	  echo '#define MODALFAD_DRIVER'; echo '#define MODALFAD_GROUP 4'; cat archs/modalfadcpu.cpp; } \
	  > build/mojo-gpu-modal-fad/modalfadcpu_main.cpp
	$(CXX_MODAL) build/mojo-gpu-modal-fad/modalfadcpu_main.cpp -o build/mojo-gpu-modal-fad/modalfadcpu
	build/mojo-gpu-modal-fad/modalfad validate
	build/mojo-gpu-modal-fad/modalfad_arena validate
mojo-gpu-modal-fad: mojo-gpu-modal-fad-build
	build/mojo-gpu-modal-fad/modalfad bench $(MODAL)
	build/mojo-gpu-modal-fad/modalfad_arena bench $(MODAL)
	build/mojo-gpu-modal-fad/modalfadcpu bench $(MODAL) $(MODAL_THREADS)
	build/mojo-gpu-modal-fad/modalfad fit $(MODAL) $(MODAL_EPOCHS) $(MODAL_SIGMA) $(MODAL_UPDATE)

# The same fit against a recording (`fit-file`, NPARAM=4): BELL_WAV, a struck
# object recorded by the user (any format sox reads; not in the repository),
# from BELL_START seconds, BELL_SECONDS after the strike. Starting modes from
# the spectrum (tools/modal_init.py, needs numpy: BELL_MODES peaks), then
# BELL_EPOCHS epochs of BELL_UPDATE at rate BELL_LR on the GPU, one instance
# of ../corpus/gpu_modal_fad_voice_phase.dsp per mode. Writes the fitted modes
# (fit.modes.txt) and target.wav, model.wav and residual.wav, all three at the
# target's gain, to build/mojo-gpu-modal-fad-bell/. Then times the same fit on
# the CPU (archs/modalfadcpufile.cpp: ../corpus/gpu_modal_fad_group_phase.dsp,
# four modes per program, clang, on MODAL_THREADS threads) against the GPU's
# epoch_ms printed by the fit.
BELL_WAV ?=
BELL_START ?= 0
BELL_SECONDS ?= 4
BELL_MODES ?= 300
BELL_EPOCHS ?= 1500
BELL_UPDATE ?= adam
BELL_LR ?= 0.05
BELL_DIR = build/mojo-gpu-modal-fad-bell
mojo-gpu-modal-fad-bell:
	@test -n "$(BELL_WAV)" || { echo "mojo-gpu-modal-fad-bell: name a recording with BELL_WAV=<file>" >&2; false; }
	cargo build --release -p compiler --example mojo_batch
	mkdir -p $(BELL_DIR)
	../../target/release/examples/mojo_batch ../corpus/gpu_modal_fad_voice_phase.dsp --cn modaldsp -o $(BELL_DIR)/modal.mojo
	$(FAUST_RS) -lang mojo -single -cn refdsp ../corpus/gpu_modal_fad_voice_phase.dsp -o $(BELL_DIR)/ref.mojo
	cat archs/impulsemojo.mojo $(BELL_DIR)/modal.mojo $(BELL_DIR)/ref.mojo archs/modalfadmojogpubatch.mojo \
	  > $(BELL_DIR)/modal_main.mojo
	$(MOJO) build $(BELL_DIR)/modal_main.mojo -o $(BELL_DIR)/modalfad -O3 -D DFAUST=DType.float32 -D NPARAM=4
	$(BELL_DIR)/modalfad validate
	sox "$(BELL_WAV)" -r 48000 -c 1 -t f32 $(BELL_DIR)/recording.f32
	python3 tools/modal_init.py $(BELL_DIR)/recording.f32 $(BELL_DIR)/bell \
	  --start $(BELL_START) --seconds $(BELL_SECONDS) --modes $(BELL_MODES)
	$(BELL_DIR)/modalfad fit-file $(BELL_DIR)/bell.target.f32 $(BELL_DIR)/bell.modes.txt $(BELL_EPOCHS) \
	  $(BELL_DIR)/fit $(BELL_UPDATE) $(BELL_LR)
	gain=`sox -t f32 -r 48000 -c 1 $(BELL_DIR)/bell.target.f32 -n stat -v 2>&1`; \
	sox -t f32 -r 48000 -c 1 $(BELL_DIR)/bell.target.f32 -b 24 $(BELL_DIR)/target.wav vol $$gain; \
	sox -t f32 -r 48000 -c 1 $(BELL_DIR)/fit.model.f32 -b 24 $(BELL_DIR)/model.wav vol $$gain; \
	sox -t f32 -r 48000 -c 1 $(BELL_DIR)/fit.residual.f32 -b 24 $(BELL_DIR)/residual.wav vol $$gain
	$(FAUST_RS) -lang cpp -single -cn plainvoice -pn plain ../corpus/gpu_modal_fad_group_phase.dsp -o $(BELL_DIR)/plain.cpp
	$(FAUST_RS) -lang cpp -single -cn fadvoice ../corpus/gpu_modal_fad_group_phase.dsp -o $(BELL_DIR)/fad.cpp
	cat archs/modalfadcpu.cpp $(BELL_DIR)/plain.cpp $(BELL_DIR)/fad.cpp archs/modalfadcpufile.cpp > $(BELL_DIR)/cpufit_main.cpp
	$(CXX_MODAL) $(BELL_DIR)/cpufit_main.cpp -o $(BELL_DIR)/cpufit
	$(BELL_DIR)/cpufit $(BELL_DIR)/bell.target.f32 $(BELL_DIR)/bell.modes.txt 8 $(MODAL_THREADS)

# The cost model of the GPU lane (G2): timings over voices x block sizes, to
# build/mojo-gpu-bench/results.csv. Runs serially (-j would skew timings).
mojo-gpu-bench:
	$(MAKE) -j1 -f Make.mojogpubench all

# The same sweep with the batch contract (G3.5): state as a structure of
# arrays across voices, emitted by the compiler's `mojo_batch` example.
mojo-gpu-bench-batch:
	cargo build --release -p compiler --example mojo_batch
	$(MAKE) -j1 -f Make.mojogpubench all BENCH_LAYOUT=batch

# Cmajor is an opt-in external lane: it needs the Cmajor command-line tool and
# the pinned C++ Faust adapter, so it is intentionally absent from `all` and the
# scalar/vector scheduling matrix.
cmajor:
	$(MAKE) -f Make.cmajor all outdir=cmajor

assemblyscript:
	$(MAKE) -f Make.assemblyscript all outdir=assemblyscript

assemblyscript-vec0:
	$(MAKE) -f Make.assemblyscript all outdir=assemblyscript-vec0 VECOPTS="-vec -lv 0"

assemblyscript-vec1:
	$(MAKE) -f Make.assemblyscript all outdir=assemblyscript-vec1 VECOPTS="-vec -lv 1"

bench:
	@$(MAKE) -f Make.bench bench

bench-self-test:
	@$(MAKE) -f Make.bench bench-self-test

vec-bench:
	@$(MAKE) -f Make.bench vec-bench

compile-bench:
	@$(MAKE) -f Make.bench compile-bench

# Build the faust-rs binaries the harness drives.
build:
	cd ../.. && cargo build --release -p compiler --bin faust-rs -p impulse-runner
	cd ../.. && cargo build --release -p cranelift-ffi --bin impulse-cranelift --bin mem0-json-check

tools: $(COMPARE)

$(COMPARE): tools/filesCompare.cpp
	$(CXX) -O2 -std=c++11 $< -o $@

clean:
	rm -rf ir build

distclean: clean
	rm -rf reference

help:
	@echo "-------- faust-rs impulse-response tests --------"
	@echo "Targets:"
	@echo "  build          : cargo build the faust-rs compiler + impulse runner"
	@echo "  reference      : preflight C++ oracle support, then generate supported .ir files"
	@echo "  reference-report: list C++-oracle-supported/rejected DSPs, diagnostic logs, library status"
	@echo "  reference-libs-accept: record the installed Faust libraries as those of refdir"
	@echo "  interp         : check the faust-rs interpreter backend (scalar prefix)"
	@echo "  cranelift      : check the faust-rs Cranelift JIT backend (scalar prefix, 64-bit)"
	@echo "  wasm           : check the faust-rs WASM backend (scalar prefix, 64-bit, Node)"
	@echo "  assemblyscript : check the faust-rs AssemblyScript backend (scalar prefix, Node/asc)"
	@echo "  rust           : check the faust-rs Rust backend (scalar prefix, rustc)"
	@echo "  julia          : check the faust-rs Julia backend (scalar prefix, Julia)"
	@echo "  mojo           : check the faust-rs Mojo backend (scalar prefix, opt-in, needs MOJO)"
	@echo "  mojo-outline   : the same with every block outlined into its own method"
	@echo "  mojo-gpu       : the same, many voices per program on the GPU (opt-in, needs MOJO)"
	@echo "  mojo-gpu-batch : mojo-gpu with the batch contract (structure of arrays)"
	@echo "  mojo-gpu-poly  : a polyphonic instrument played in real time (needs MOJO, PortAudio)"
	@echo "  mojo-gpu-bells : a carillon, 50 modes per bell on the GPU (needs MOJO, PortAudio)"
	@echo "  mojo-gpu-choir : an additive choir, ~48,000 rich partials on the GPU (needs MOJO, PortAudio)"
	@echo "  mojo-gpu-choir-reverb : the same choir in a hall, the reverb on the CPU (needs MOJO, PortAudio)"
	@echo "  mojo-gpu-choir-render : both choir versions rendered offline to WAV files"
	@echo "  mojo-gpu-modal-fad : fits thousands of modes to a target by fad on the GPU, against the CPU (needs MOJO)"
	@echo "  mojo-gpu-modal-fad-bell : the same fit against a recording, BELL_WAV=<file> (needs MOJO, sox, numpy)"
	@echo "  mojo-gpu-bench : GPU against CPU timings for many voices (opt-in, needs MOJO)"
	@echo "  mojo-gpu-bench-batch : the same with the batch layout (structure of arrays)"
	@echo "  cmajor         : check scalar Cmajor via cmaj-generated C++ (external)"
	@echo "  *-vec0/*-vec1  : check a backend with -vec -lv 0 / -vec -lv 1"
	@echo "  all-vec        : run all vector-mode backend variants"
	@echo "  <backend>-ssN  : check scalar mode with -ss N (N=0..3)"
	@echo "  <backend>-vecL-ssN: check -vec -lv L crossed with -ss N"
	@echo "  all-ss         : scalar -ss 0..3 across all eight backends"
	@echo "  all-vec-ss     : -lv 0/1 x -ss 0..3 across all eight backends"
	@echo "  <backend>-ec/-os/-ec-os: check the execution options (cpp/c/rust only)"
	@echo "  all-execopts   : the nine execution-option shapes (self-contained arch)"
	@echo "  cpp-mem0/c-mem0/cranelift-mem0: check -mem0 on the full supported dsp/ corpus"
	@echo "  *-mem0-smoke   : audit one mem0 backend on the focused dsp-mem0/ corpus"
	@echo "  mem0-smoke     : audit all mem0 backends, JSON, and O0/O3 on dsp-mem0/"
	@echo "  all-mem0       : full supported corpus plus the focused mem0 audit"
	@echo "  mem0-opt       : rerun the focused mem0 O0/O3 parity matrix"
	@echo "  mem0-sanitize  : run the focused C/C++ mem0 audit with ASan and UBSan"
	@echo "  rad-reference  : build the RAD lane reference (faust-rs interpreter)"
	@echo "  rad-<backend>  : check RAD gradients against that reference"
	@echo "  all-rad        : every backend that emits RAD code (needs rad-reference)"
	@echo "  backend-matrix : all-ss + all-vec-ss (full configured corpus)"
	@echo "  backend-matrix-smoke: backend-matrix on BACKEND_MATRIX_SMOKE_DSPFILES"
	@echo "  backend-matrix-full : clean matrix outputs, run full matrix, write audited report"
	@echo "  bench          : compare generated-code speed (usual: BENCH_OPTIONS='-double -run 5 -bs 512')"
	@echo "  bench-self-test: validate benchmark ordering, statuses, and aggregate calculations"
	@echo "  vec-bench      : compare scalar/vec0/vec1 C++ code under -ss 0..3 for universally certified DSPs"
	@echo "  compile-bench  : compare C++ Faust and faust-rs compile time on the DSP corpus"
	@echo "  cpp            : check the faust-rs C++ backend (full 4-pass)"
	@echo "  c              : check the faust-rs C backend (full 4-pass)"
	@echo "  all            : scalar corpus across all eight backends"
	@echo "  tools          : build the filesCompare comparator"
	@echo "  clean          : remove ir/ and build/"
	@echo "  distclean      : also remove reference/"
	@echo ""
	@echo "Key variables (see common.mk, all overridable):"
	@echo "  FAUST_CPP=$(FAUST_CPP)"   
	@echo "  CPP_TESTS=$(CPP_TESTS)"
	@echo "  FAUST_ARCH=$(FAUST_ARCH)"
	@echo "  FAUST_RS=$(FAUST_RS)" 
	@echo "  RUNNER=$(RUNNER)"
	@echo "  FAUSTBENCH=$(FAUSTBENCH)"   
	@echo "  BENCH_OPTIONS=$(BENCH_OPTIONS)"
	@echo "  BENCH_DIR=$(BENCH_DIR)"
	@echo "  BENCH_CSV=$(BENCH_CSV)"
	@echo "  BENCH_AGGREGATE_CSV=$(BENCH_AGGREGATE_CSV)"
	@echo "  VEC_BENCH_OPTIONS=$(VEC_BENCH_OPTIONS)"
	@echo "  VEC_BENCH_CSV=$(VEC_BENCH_CSV)"
	@echo "  VEC_BENCH_SUMMARY_CSV=$(VEC_BENCH_SUMMARY_CSV)"
	@echo "  VEC_BENCH_AGGREGATE_CSV=$(VEC_BENCH_AGGREGATE_CSV)"
	@echo "  COMPILE_BENCH_CSV=$(COMPILE_BENCH_CSV)"
	@echo "  CMAJ_BIN=cmaj"
	@echo "  CMAJ_CXX=$(CXX)"
	@echo "  precision=<tol>  (filesCompare tolerance override)"
	@echo ""
	@echo "Differential backend targets refresh the C++ oracle automatically."
