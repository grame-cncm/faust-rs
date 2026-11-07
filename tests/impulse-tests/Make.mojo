# Test the faust-rs Mojo backend against the C++ impulse reference.
#
# The generated `-lang mojo -double` source is concatenated between the local,
# self-contained impulse runtime (`archs/impulsemojo.mojo`) and its entry point
# (`archs/runimpulsemojo.mojo`), built with `mojo build`, and run. The harness
# mirrors the scalar prefix of the Julia, Rust and interpreter runners.
#
# Opt-in: it needs the Mojo toolchain, which CI does not install. `MOJO` names
# the command, for instance the locked environment of grame-cncm/faust#1293:
#   make mojo MOJO="pixi run --manifest-path <faust>/architecture/mojo/pixi.toml -q mojo"

include common.mk

outdir ?= mojo
ARCH_MOJO ?= archs/impulsemojo.mojo
ARCH_MOJO_TAIL ?= archs/runimpulsemojo.mojo
MOJO ?= mojo
MOJOBUILDOPTIONS ?= -O3 -D DFAUST=DType.float64

allnames := $(dspfiles:$(dspdir)/%.dsp=%)
testnames := $(filter-out $(call known_fail_for,$(outdir)),$(allnames))
listfiles := $(testnames:%=ir/$(outdir)/%.ir)

.PHONY: all
.DELETE_ON_ERROR:

all: $(COMPARE) ir/$(outdir)/.dir build/$(outdir)/.dir $(listfiles)

ir/$(outdir)/.dir:
	mkdir -p ir/$(outdir)
	touch $@

build/$(outdir)/.dir:
	mkdir -p build/$(outdir)
	touch $@

$(COMPARE): tools/filesCompare.cpp
	$(CXX) -O2 -std=c++11 $< -o $@

ir/$(outdir)/%.ir: $(dspdir)/%.dsp $(refdir)/%.ir $(ARCH_MOJO) $(ARCH_MOJO_TAIL) | ir/$(outdir)/.dir build/$(outdir)/.dir
	$(FAUST_RS) -lang mojo -double -cn mydsp $(COMPILER_OPTS) -I $(dspdir) -I $(FAUSTLIBS) $< -o build/$(outdir)/$*.mojo
	cat $(ARCH_MOJO) build/$(outdir)/$*.mojo $(ARCH_MOJO_TAIL) > build/$(outdir)/$*_main.mojo
	$(MOJO) build build/$(outdir)/$*_main.mojo -o build/$(outdir)/$* $(MOJOBUILDOPTIONS)
	build/$(outdir)/$* > $@
	$(COMPARE) $@ $(refdir)/$*.ir -part $(call dsp_precision,$*) || (rm -f $@; false)
