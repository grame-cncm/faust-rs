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

.PHONY: all all-vec0 all-vec1 all-vec all-ss all-vec-ss backend-matrix backend-matrix-smoke backend-matrix-clean backend-matrix-report backend-matrix-full reference reference-report interp interp-vec0 interp-vec1 c c-vec0 c-vec1 cpp cpp-vec0 cpp-vec1 cranelift cranelift-vec0 cranelift-vec1 wasm wasm-vec0 wasm-vec1 assemblyscript assemblyscript-vec0 assemblyscript-vec1 rust rust-vec0 rust-vec1 julia julia-vec0 julia-vec1 cmajor cpp-ec cpp-os cpp-ec-os c-ec c-os c-ec-os rust-ec rust-os rust-ec-os all-execopts cpp-mem0 c-mem0 cranelift-mem0 all-mem0 cpp-mem0-smoke c-mem0-smoke cranelift-mem0-smoke mem0-smoke mem0-opt mem0-sanitize bench bench-self-test vec-bench compile-bench build tools help clean distclean

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
	cd ../.. && cargo build --release -p cranelift-ffi --bin impulse_cranelift --bin mem0_json_check

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
	@echo "  reference-report: list C++-oracle-supported/rejected DSPs and diagnostic logs"
	@echo "  interp         : check the faust-rs interpreter backend (scalar prefix)"
	@echo "  cranelift      : check the faust-rs Cranelift JIT backend (scalar prefix, 64-bit)"
	@echo "  wasm           : check the faust-rs WASM backend (scalar prefix, 64-bit, Node)"
	@echo "  assemblyscript : check the faust-rs AssemblyScript backend (scalar prefix, Node/asc)"
	@echo "  rust           : check the faust-rs Rust backend (scalar prefix, rustc)"
	@echo "  julia          : check the faust-rs Julia backend (scalar prefix, Julia)"
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
