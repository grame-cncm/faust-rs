#!/bin/sh
# Regenerates every C++ oracle capture from corpus/ into captures/.
# Reference: Faust C++ branch master-dev-ocpp-od-fir-2-FIR19, commit 8eebea429.
# Override the binary with FAUST_CPP_BIN.
set -u
FAUST=${FAUST_CPP_BIN:-/Users/letz/Developpements/RUST/faust/build/bin/faust}
HERE=$(cd "$(dirname "$0")" && pwd)
CORPUS="$HERE/corpus"
OUT="$HERE/captures"
mkdir -p "$OUT"

# run <case> <config-name> <flags...>: capture stdout on success, the error
# message as <case>.<config>.err on failure (a rejection IS an oracle fact).
run() {
  case_name=$1; config=$2; shift 2
  if "$FAUST" "$@" "$CORPUS/$case_name.dsp" > "$OUT/$case_name.$config.cpp" 2> "$OUT/.stderr.$$"; then
    rm -f "$OUT/$case_name.$config.err"
  else
    mv "$OUT/.stderr.$$" "$OUT/$case_name.$config.err"
    rm -f "$OUT/$case_name.$config.cpp"
  fi
  rm -f "$OUT/.stderr.$$"
}

# phs <case> <n>: hierarchical schedule print for -ss <n>. The schedule is
# printed on STDERR by this build; generated code on stdout is discarded.
phs() {
  case_name=$1; n=$2
  "$FAUST" -phs -ss "$n" -lang cpp "$CORPUS/$case_name.dsp" \
    > /dev/null 2> "$OUT/$case_name.phs_ss$n.txt" || true
}

for f in "$CORPUS"/*.dsp; do
  c=$(basename "$f" .dsp)
  run "$c" scalar        -lang cpp
  run "$c" scalar_ss1    -lang cpp -ss 1
  run "$c" scalar_ss2    -lang cpp -ss 2
  run "$c" scalar_ss3    -lang cpp -ss 3
  run "$c" vec_lv0       -lang cpp -vec -lv 0
  run "$c" vec_lv1       -lang cpp -vec -lv 1
  run "$c" vec_dfs       -lang cpp -vec -dfs
  phs "$c" 0
  phs "$c" 1
  phs "$c" 2
  phs "$c" 3
done

# double-precision variants where numerics matter most
for c in short_delay long_delay table_rw rec_multi_delay; do
  run "$c" scalar_double -lang cpp -double
  run "$c" vec_lv0_double -lang cpp -double -vec -lv 0
done

echo "captures regenerated in $OUT"
