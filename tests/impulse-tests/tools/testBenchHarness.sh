#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
impulse_dir=$(dirname "$script_dir")
scratch=$(mktemp -d "${TMPDIR:-/tmp}/faust-rs-bench-test.XXXXXX")
trap 'rm -rf "$scratch"' EXIT HUP INT TERM

fake_bench="$scratch/fake-faustbench"
trace="$scratch/trace"
summary="$scratch/summary.csv"
aggregate="$scratch/aggregate.csv"

cat > "$fake_bench" <<'EOF'
#!/bin/sh
set -eu

compiler=$(command -v faust)
case "$compiler" in
    *cpp-bin/faust) side=cpp ;;
    *rs-bin/faust) side=rust ;;
    *) exit 2 ;;
esac

for arg in "$@"; do
    case "$arg" in
        dsp/*.dsp) name=${arg#dsp/}; name=${name%.dsp} ;;
    esac
done
printf '%s,%s\n' "$name" "$side" >> "$BENCH_TEST_TRACE"

case "$name,$side" in
    faster,cpp|slower,cpp|same,cpp|unsupported,rust|failed_cpp,rust|failed_rs,cpp|nonfinite_cpp,rust|nonfinite_rs,cpp) value=100 ;;
    faster,rust) value=125 ;;
    slower,rust) value=80 ;;
    same,rust) value=100 ;;
    unsupported,cpp)
        printf 'dsp/unsupported.dsp:1 : ERROR : undefined symbol : ondemand\n' >&2
        exit 1
        ;;
    failed_cpp,cpp|failed_rs,rust|failed_both,cpp|failed_both,rust)
        printf 'synthetic %s failure\n' "$side" >&2
        exit 1
        ;;
    nonfinite_cpp,cpp|nonfinite_both,cpp) value=inf ;;
    nonfinite_rs,rust|nonfinite_both,rust) value=nan ;;
    *) exit 3 ;;
esac

printf 'Best value is : %s MBytes/sec, SD : 0%%\n' "$value"
EOF
chmod +x "$fake_bench"

BENCH_TEST_TRACE="$trace" make -s -f "$impulse_dir/Make.bench" \
    -C "$impulse_dir" bench \
    dspfiles="dsp/faster.dsp dsp/slower.dsp dsp/same.dsp dsp/unsupported.dsp dsp/failed_cpp.dsp dsp/failed_rs.dsp dsp/failed_both.dsp dsp/nonfinite_cpp.dsp dsp/nonfinite_rs.dsp dsp/nonfinite_both.dsp" \
    FAUST_CPP=/bin/true FAUST_RS=/bin/true \
    FAUSTBENCH="$fake_bench" FAUSTLIBS="$scratch" \
    BENCH_DIR="$scratch/build" BENCH_CSV="$summary" \
    BENCH_AGGREGATE_CSV="$aggregate" >"$scratch/harness.stdout" 2>"$scratch/harness.stderr"

expected_trace="$scratch/expected-trace"
cat > "$expected_trace" <<'EOF'
faster,cpp
faster,rust
slower,rust
slower,cpp
same,cpp
same,rust
unsupported,rust
unsupported,cpp
failed_cpp,cpp
failed_cpp,rust
failed_rs,rust
failed_rs,cpp
failed_both,cpp
failed_both,rust
nonfinite_cpp,rust
nonfinite_cpp,cpp
nonfinite_rs,cpp
nonfinite_rs,rust
nonfinite_both,rust
nonfinite_both,cpp
EOF
cmp "$expected_trace" "$trace"

expected_summary="$scratch/expected-summary.csv"
cat > "$expected_summary" <<'EOF'
dsp,faust_cpp_mbytes_sec,faust_rs_mbytes_sec,delta_pct,status,run_order
faster,100,125,25.00,ok,cpp-first
slower,100,80,-20.00,ok,faust-rs-first
same,100,100,0.00,ok,cpp-first
unsupported,,100,,unsupported_cpp,faust-rs-first
failed_cpp,,100,,failed_cpp,cpp-first
failed_rs,100,,,failed_faust_rs,faust-rs-first
failed_both,,,,failed_both,cpp-first
nonfinite_cpp,inf,100,,nonfinite_cpp,faust-rs-first
nonfinite_rs,100,nan,,nonfinite_faust_rs,cpp-first
nonfinite_both,inf,nan,,nonfinite_both,faust-rs-first
EOF
cmp "$expected_summary" "$summary"

expected_aggregate="$scratch/expected-aggregate.csv"
cat > "$expected_aggregate" <<'EOF'
comparable_dsps,better,worse,same,geomean_delta_pct,median_delta_pct,regressions_ge_warn,unsupported_cpp,failed_cpp,failed_faust_rs,failed_both,nonfinite_cpp,nonfinite_faust_rs,nonfinite_both
3,1,1,1,0.00,0.00,1,1,1,1,1,1,1,1
EOF
cmp "$expected_aggregate" "$aggregate"

grep -q 'summary: comparable=3 better=1 worse=1 same=1 geomean=0.00% median=0.00%' "$scratch/harness.stdout"
for status in unsupported_cpp failed_cpp failed_faust_rs failed_both nonfinite_cpp nonfinite_faust_rs nonfinite_both; do
    grep -q "\\[bench\\]\\[$status\\]" "$scratch/harness.stderr"
done

printf 'bench harness self-test: ok\n'
