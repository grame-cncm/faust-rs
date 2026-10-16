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
RUNNER_CRANELIFT ?= ../../target/release/impulse-cranelift

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
# `-fwrapv`: Faust-generated code assumes two's-complement wrapping for the
# integer noise LCG (`1103515245 * i + 12345` overflows `int`, which is UB
# in C/C++). Apple clang 21 started exploiting that UB at -O2/-O3 — the
# fill-loop variant miscompiles (caught by `make cpp` on table.dsp, where
# the C++ reference, our interpreter, and our -O0/-fwrapv builds all agree
# and only the optimized no-wrapv build diverges). Applies to reference
# builds too: rebuilding the reference without it produces the same wrong
# values.
CXXFLAGS ?= -O3 -fwrapv -I$(FAUST_ARCH) -I$(CPP_TESTS)/archs -pthread -std=c++11
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
# Backend-matrix scheduling targets use this for scalar `-ss N` and vector
# `-vec -lv N -ss M` combinations. Command-line VECOPTS still propagates.
COMPILER_OPTS ?= $(VECOPTS)

# --- performance benchmark --------------------------------------------------
# `faustbench` invokes a `faust` binary found on PATH, so Make.bench creates
# temporary PATH wrappers around FAUST_CPP and FAUST_RS.
FAUSTBENCH ?= faustbench -single
BENCH_OPTIONS ?= -double
BENCH_WARN_MIN ?= 5
BENCH_DIR ?= build/bench
BENCH_CSV ?= $(BENCH_DIR)/summary.csv
BENCH_AGGREGATE_CSV ?= $(BENCH_DIR)/aggregate.csv
VEC_BENCH_OPTIONS ?= $(BENCH_OPTIONS)
VEC_BENCH_WARN_MIN ?= 5
VEC_BENCH_CSV ?= $(BENCH_DIR)/vector-scheduling.csv
VEC_BENCH_SUMMARY_CSV ?= $(BENCH_DIR)/vector-scheduling-summary.csv
VEC_BENCH_AGGREGATE_CSV ?= $(BENCH_DIR)/vector-scheduling-aggregate.csv
COMPILE_BENCH_CSV ?= $(BENCH_DIR)/compile-summary.csv

# Which DSP set is under test, and where its reference responses live.
#
# The default pair (`dsp/` + `reference/`) is the ordinary suite, whose
# reference is produced by the genuine C++ Faust compiler (`Make.ref`).
#
# The pair exists because not every faust-rs program *has* a C++ Faust
# reference: `rad` and `fad` are faust-rs primitives, and C++ Faust rejects
# them with `undefined symbol : rad`. Those programs live in `dsp-rad/` and
# take their reference from the faust-rs interpreter instead (`Make.ref-rad`),
# which every backend lane then compares against unchanged.
dspdir ?= dsp
refdir ?= reference

dspfiles := $(wildcard $(dspdir)/*.dsp)
# `Make.ref` regenerates this manifest by asking the configured C++ Faust
# compiler whether each selected DSP can produce the normal C++ oracle. It is
# deliberately separate from `KNOWN_FAIL_*`: an unavailable C++ oracle makes a
# differential comparison impossible but says nothing about faust-rs.
CPP_ORACLE_MANIFEST ?= build/ref/cpp-oracle-manifest.mk
CPP_ORACLE_CONFIG ?= build/ref/cpp-oracle-config.txt
CPP_ORACLE_LOG_DIR ?= build/ref/cpp-oracle-errors
ifneq ($(wildcard $(CPP_ORACLE_MANIFEST)),)
include $(CPP_ORACLE_MANIFEST)
endif
CPP_ORACLE_SUPPORTED ?=
CPP_ORACLE_UNSUPPORTED ?=
# The manifest records bare DSP names. Filter by full path so `dspdir` remains
# overridable for targeted and alternate-corpus runs.
cpp_oracle_dspfiles = $(filter-out $(addprefix $(dspdir)/,$(addsuffix .dsp,$(CPP_ORACLE_UNSUPPORTED))),$(dspfiles))
VECTOR_CERTIFIED_LIST := ../vector-coverage/certified-dspfiles.txt
vector_certified_repo_files := $(shell sed -n '/\.dsp$$/p' $(VECTOR_CERTIFIED_LIST) 2>/dev/null)
vector_certified_dspfiles := $(patsubst tests/impulse-tests/%,%,$(vector_certified_repo_files))

# Per-DSP tolerance overrides and known-failure lists.
include known.mk

# --- Faust-library provenance of the references ----------------------------
# A reference response depends on the Faust libraries as much as on its DSP,
# but Make only tracks the DSP: after `$(FAUSTLIBS)` is reinstalled every
# reference still looks up to date, and a DSP whose library code changed fails
# on every backend as if faust-rs had regressed (`phaser_flanger`, 2026-10-16,
# after faustlibraries 1235d855 and 649a5ecd).
#
# `$(FAUSTLIBS_STAMP)` records the `cksum` of every `$(FAUSTLIBS)/*.lib` a
# reference set was produced with. `Make.ref` and `Make.ref-rad` write it when
# they create an empty reference directory; afterwards only
# `make reference-libs-accept` rewrites it. A library change never regenerates
# a reference by itself (`sound.ir` needs a specific C++ fixture, see
# README.md). When the record is missing or differs from the installed
# libraries, make warns once per reference set and process tree, naming the
# changed files.
FAUSTLIBS_STAMP ?= $(refdir)/.faustlibs
FAUSTLIBS_MANIFEST_CMD = (cd '$(FAUSTLIBS)' && LC_ALL=C cksum *.lib)
# The library files whose record line differs from the installed one (changed,
# added or removed).
faustlibs_changed = $(shell { cat '$(FAUSTLIBS_STAMP)'; $(FAUSTLIBS_MANIFEST_CMD); } 2>/dev/null | LC_ALL=C sort | uniq -u | awk '{ print $$3 }' | LC_ALL=C sort -u)
# `$(warning)` strips leading blanks; this keeps the continuation lines indented.
faustlibs_indent := $(empty)  $(empty)
faustlibs_accept_cmd = make reference-libs-accept$(if $(filter-out reference,$(refdir)), refdir=$(refdir))

# Goals that never compare against a reference stay quiet.
faustlibs_quiet_goals := clean distclean help build tools reference-libs-accept \
	bench bench-self-test vec-bench compile-bench
faustlibs_goals := $(if $(MAKECMDGOALS),$(filter-out $(faustlibs_quiet_goals),$(MAKECMDGOALS)),default)
ifneq ($(faustlibs_goals),)
ifeq ($(filter $(refdir),$(FAUSTLIBS_CHECKED_REFDIRS)),)
ifneq ($(wildcard $(refdir)/*.ir),)
ifeq ($(wildcard $(FAUSTLIBS_STAMP)),)
$(warning WARNING: $(refdir)/ has no record of the Faust libraries it was produced with ($(FAUSTLIBS_STAMP)).)
$(warning $(faustlibs_indent)A mismatch against $(refdir)/ may come from a library change, not from faust-rs.)
$(warning $(faustlibs_indent)Once its references are known to match $(FAUSTLIBS): $(faustlibs_accept_cmd))
else
faustlibs_changes := $(faustlibs_changed)
ifneq ($(faustlibs_changes),)
$(warning WARNING: $(refdir)/ was produced with other Faust libraries than $(FAUSTLIBS) holds now.)
$(warning $(faustlibs_indent)Changed: $(faustlibs_changes))
$(warning $(faustlibs_indent)A mismatch against $(refdir)/ may come from these libraries, not from faust-rs.)
$(warning $(faustlibs_indent)Regenerate the references they affect, then: $(faustlibs_accept_cmd))
endif
endif
endif
endif
endif
# Exported, so the sub-makes of one invocation do not repeat the warning.
FAUSTLIBS_CHECKED_REFDIRS := $(FAUSTLIBS_CHECKED_REFDIRS) $(refdir)
export FAUSTLIBS_CHECKED_REFDIRS
