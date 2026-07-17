#!/bin/sh
# Regenerates src/dx7.rs from the Faust DSP with the faust-rs Rust backend.
set -e
cd "$(dirname "$0")"
FAUST_RS=${FAUST_RS:-../../../target/release/faust-rs}
FAUSTLIBS=${FAUSTLIBS:-/opt/homebrew/share/faust}
"$FAUST_RS" -lang rust -cn Dx7Piano -I "$FAUSTLIBS" dsp/dx7_alg5.dsp -o src/dx7.rs
echo "regenerated src/dx7.rs"
