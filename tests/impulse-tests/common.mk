#
# Shared configuration for the faust-rs impulse-response test machinery.
#
# Every variable is overridable from the environment or the make command line,
# e.g. `make interp FAUST_RS=/path/to/faust-rs`.
#

# --- faust-rs side (the system under test) ---------------------------------
# The faust-rs compiler binary and the interpreter impulse runner. Built from
# the workspace with `cargo build --release -p compiler -p impulse-runner`.
FAUST_RS  ?= ../../target/release/faust-rs
RUNNER    ?= ../../target/release/impulse-runner
RUNNER_CRANELIFT ?= ../../target/release/impulse_cranelift

# --- C++ reference oracle ---------------------------------------------------
# The reference `.ir` files are produced by the genuine C++ Faust compiler
# wrapped in the original 4-pass impulse architecture. That architecture pulls
# headers from a C++ Faust checkout, so reference generation and the native
# C/C++ test paths depend on it. Point these at your Faust source tree.
FAUST_CPP  ?= faust
CPP_TESTS  ?= /Users/letz/Developpements/RUST/faust/tests/impulse-tests
FAUST_ARCH ?= /Users/letz/Developpements/RUST/faust/architecture
FAUST_INCLUDE_DIR ?= $(shell $(FAUST_CPP) -includedir 2>/dev/null || printf /usr/local/include)
FAUST_LIB_DIR ?= $(shell $(FAUST_CPP) -libdir 2>/dev/null || printf /usr/local/lib)
IMPULSE_ARCH ?= $(CPP_TESTS)/archs/impulsearch.cpp
# The C backend emits C functions wrapped by a C++ `Cdsp` adaptor, so it uses a
# dedicated impulse architecture.
IMPULSE_ARCH_C ?= $(CPP_TESTS)/archs/impulsearch2.cpp

# Faust standard libraries (auto-resolved by the C++ compiler, must be passed
# explicitly to faust-rs which does not add system paths when -I is given).
FAUSTLIBS ?= /usr/local/share/faust

# --- native build / comparison ---------------------------------------------
CXX      ?= c++
CXXFLAGS ?= -O3 -I$(FAUST_ARCH) -I$(CPP_TESTS)/archs -pthread -std=c++11
COMPARE  ?= ./tools/filesCompare

# Total reference frames (4 passes of 15000) and the scalar-only prefix the
# faust-rs interpreter/JIT runners can reproduce today.
NFRAMES      ?= 60000
SCALARFRAMES ?= 15000

# filesCompare tolerance override (empty -> default 2e-06).
precision ?=

# Compatibility alias retained for the original vector-only targets.
VECOPTS ?=
# Extra faust-rs / runner options injected into every backend invocation.
# P7 scheduling targets use this for scalar `-ss N` and vector
# `-vec -lv N -ss M` combinations. Command-line VECOPTS still propagates.
COMPILER_OPTS ?= $(VECOPTS)

# --- performance benchmark --------------------------------------------------
# `faustbench` invokes a `faust` binary found on PATH, so Make.bench creates
# temporary PATH wrappers around FAUST_CPP and FAUST_RS.
FAUSTBENCH ?= faustbench -single
BENCH_OPTIONS ?= -double
BENCH_WARN_MIN ?= 5
BENCH_CSV ?= build/bench/summary.csv
VEC_BENCH_OPTIONS ?= $(BENCH_OPTIONS)
VEC_BENCH_WARN_MIN ?= 5
VEC_BENCH_CSV ?= build/bench/vector-scheduling.csv
VEC_BENCH_SUMMARY_CSV ?= build/bench/vector-scheduling-summary.csv
VEC_BENCH_AGGREGATE_CSV ?= build/bench/vector-scheduling-aggregate.csv
COMPILE_BENCH_CSV ?= build/bench/compile-summary.csv

dspfiles := $(wildcard dsp/*.dsp)
VECTOR_CERTIFIED_LIST := ../vector-coverage/certified-dspfiles.txt
vector_certified_repo_files := $(shell sed -n '/\.dsp$$/p' $(VECTOR_CERTIFIED_LIST) 2>/dev/null)
vector_certified_dspfiles := $(patsubst tests/impulse-tests/%,%,$(vector_certified_repo_files))

# Per-DSP tolerance overrides and known-failure lists.
include known.mk
