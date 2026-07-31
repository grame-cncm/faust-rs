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

.PHONY: all all-vec0 all-vec1 all-vec all-ss all-vec-ss backend-matrix backend-matrix-smoke backend-matrix-clean backend-matrix-report backend-matrix-full reference interp interp-vec0 interp-vec1 c c-vec0 c-vec1 cpp cpp-vec0 cpp-vec1 cranelift cranelift-vec0 cranelift-vec1 wasm wasm-vec0 wasm-vec1 assemblyscript assemblyscript-vec0 assemblyscript-vec1 rust rust-vec0 rust-vec1 julia julia-vec0 julia-vec1 cpp-ec cpp-os cpp-ec-os c-ec c-os c-ec-os rust-ec rust-os rust-ec-os all-execopts bench bench-self-test vec-bench compile-bench build tools help clean distclean

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
$(1)-ss%:
	$$(MAKE) -f $(2) all outdir=$$@ $(3) COMPILER_OPTS="-ss $$*"

$(1)-vec0-ss%:
	$$(MAKE) -f $(2) all outdir=$$@ $(3) COMPILER_OPTS="-vec -lv 0 -ss $$*"

$(1)-vec1-ss%:
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
	cd ../.. && cargo build --release -p compiler -p impulse-runner
	cd ../.. && cargo build --release -p cranelift-ffi --bin impulse_cranelift

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
	@echo "  reference      : generate the genuine C++ reference .ir files (oracle)"
	@echo "  interp         : check the faust-rs interpreter backend (scalar prefix)"
	@echo "  cranelift      : check the faust-rs Cranelift JIT backend (scalar prefix, 64-bit)"
	@echo "  wasm           : check the faust-rs WASM backend (scalar prefix, 64-bit, Node)"
	@echo "  assemblyscript : check the faust-rs AssemblyScript backend (scalar prefix, Node/asc)"
	@echo "  rust           : check the faust-rs Rust backend (scalar prefix, rustc)"
	@echo "  julia          : check the faust-rs Julia backend (scalar prefix, Julia)"
	@echo "  *-vec0/*-vec1  : check a backend with -vec -lv 0 / -vec -lv 1"
	@echo "  all-vec        : run all vector-mode backend variants"
	@echo "  <backend>-ssN  : check scalar mode with -ss N (N=0..3)"
	@echo "  <backend>-vecL-ssN: check -vec -lv L crossed with -ss N"
	@echo "  all-ss         : scalar -ss 0..3 across all eight backends"
	@echo "  all-vec-ss     : -lv 0/1 x -ss 0..3 across all eight backends"
	@echo "  <backend>-ec/-os/-ec-os: check the execution options (cpp/c/rust only)"
	@echo "  all-execopts   : the nine execution-option shapes (self-contained arch)"
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
	@echo "  precision=<tol>  (filesCompare tolerance override)"
	@echo ""
	@echo "Run 'make reference' once before the backend targets."
