# `ma.SR` above 192 kHz: unclamped in `platform.lib`, bounded by typing

Date: 2026-09-28
Branch: `sr-max-sample-rate`, from `main-dev` (`d59bfe14`)
Issue: [grame-cncm/faust#1321](https://github.com/grame-cncm/faust/issues/1321)
("Make ma.SR the true sample rate at any rate, and bound delay-line allocation
instead", Julius O. Smith)
Libraries: faustlibraries `check-cpu` (`9c421426`), 57 test files, 1445 `*_test`
definitions
Status: **prototype**. The typing change is behind an environment variable, has
no CLI option and no unit test yet. The measurements below were taken with the
same patch on `rust-facade` (`c23c13de`). On `main-dev`, the patch applies
unchanged and builds, but the corpus was not re-run.

## 1. The problem

`platform.lib` defines the sample rate every library uses:

```faust
SR = min(192000.0, max(1.0, fconstant(int fSamplingFreq, <math.h>)));
```

Above 192 kHz the samples still arrive at `fSamplingFreq`, but every design
formula (filters, pre-warping, wave digital port resistances, DC blockers...)
computes with 192000. Every designed frequency then comes out scaled by
`fSamplingFreq / 192000`, and nothing warns. Hosts that oversample a Faust DSP,
for example a JUCE `dsp::Oversampling` region around a nonlinear
virtual-analog model, call `init()` at 384 or 768 kHz and get wrong filters.

The clamp exists for memory. Delay lines sized from `SR` (`SR` samples for one
second) are allocated for the largest `SR` the compiler can infer, and the
clamp is what bounds it. That argument was given in faustlibraries#5 (2018) and
faust#1149 (2025).

### 1.1 The same clamp defeats `upsampling`

Inside `upsampling(C)` with clock `H`, `faust-rs` adapts the sample rate in
`adapt_sampling_frequency` (`crates/propagate/src/engine.rs:1017`). It rewrites
the **raw** `fSamplingFreq` foreign constant to `fSamplingFreq * H`, or
`/ H` for `downsampling`, composing nested domains. The compiler does not know
which expression is `ma.SR`, so `platform.lib`'s clamp applies **after** the
factor. With `faust-rs` 0.8.0:

```faust
process = (8, _) : upsampling(fi.lowpass(2, 1000));
```

```cpp
fConst0 = std::tan(3141.5927f / std::fmin(192000.0f, std::fmax(1.0f, (float)(fSampleRate * 8))));
```

At 48 kHz × 8 the domain runs at 384 kHz but sees `ma.SR = 192000`, so the
lowpass cuts at 2 kHz. This is the most common oversampling case, and it hits
the clamp.

The local C++ `ondemand` branches (`new-new-graph-ondemand`,
`md-graph-ondemand-1`) do not adapt `fSamplingFreq` in `propagate.cpp`'s
`BoxFConst` case at all. I could not locate the C++ counterpart that the doc
comment of `adapt_sampling_frequency` says it mirrors.

## 2. Two ways to fix it

### 2.1 Option A: apply the factor after the clamp, keeping the clamp

The compiler cannot recognize the clamp in the signal tree without fragile
pattern matching. The alternative is a second, **non-adapted** host-rate
foreign constant, with `SR` rewritten as
`min(192000, max(1, host)) * (fSamplingFreq / host)`. The ratio is exactly `H`
in a domain and `1` outside.

- The name `"fSamplingFreq" | "fSamplingRate"` is special-cased in about seven
  places, each of which would need the new name:
  - `crates/propagate/src/engine.rs:444`;
  - `crates/transform/src/signal_fir/siggen.rs:133`;
  - `crates/transform/src/signal_fir/module/core_lowering.rs:458`;
  - `crates/transform/src/signal_fir/vector/lower/signal.rs:2155`;
  - `crates/compiler/src/lib.rs:1947`;
  - `crates/codegen/src/backends/interp/compiler/storage.rs:77`;
  - the Cranelift runtime.
- The C++ compiler must accept the same name, or faustlibraries stops
  compiling with it.
- It leaves the host-driven case (faust#1321's own case) broken, since the
  clamp stays.

Not pursued.

### 2.2 Option B, prototyped: remove the clamp and bound the interval in the compiler

This is item 3 of the issue. The ordering question disappears:

1. **`platform.lib`**: `SR = max(1.0, fconstant(int fSamplingFreq, <math.h>));`.
   Inside a domain, `SR = max(1, fSamplingFreq * H)`, which is right.
2. **Typing**: `infer_foreign_const_type`
   (`crates/sigtype/src/rules.rs:1771`) gives foreign constants the fully open
   interval of C++ `inferFConstType`. The sampling-frequency constants get
   `[1, R]` instead, where `R` is the maximum sample rate assumed for
   allocation (default 192000).
3. **Delay sizing is unchanged.** `delay_size_for_amount`
   (`crates/transform/src/signal_fir/delay/sizing.rs:151`) already takes the
   interval upper bound of a delay amount (its strategy 2). Interval algebra
   carries `[1, R]` through `SR * seconds`, and through `H * SR` in an
   `upsampling` domain with a constant clock, so the domain's delays are sized
   for `R * H` with no extra code.

With `R = 192000`, the interval is exactly what the clamp produced. Allocations
and values below 192 kHz are the same by construction; section 4 checks it.

## 3. The prototype

One function of `crates/sigtype/src/rules.rs` changes. The `FConst` arm passes
the constant's name, and `infer_foreign_const_type` returns `[1, R]`
(`lsb = 0`, an integer) for `fSamplingFreq`/`fSamplingRate` when
`FAUST_RS_MAX_SAMPLE_RATE=R` is set, and the open interval otherwise:

```rust
let range = match std::env::var("FAUST_RS_MAX_SAMPLE_RATE")
    .ok()
    .and_then(|v| v.parse::<f64>().ok())
{
    Some(r) if matches!(tlib::tree_to_str(self.arena, name),
                        Some("fSamplingFreq" | "fSamplingRate")) => {
        interval::Interval::new(1.0, r, 0)
    }
    _ => interval::Interval::new_default(),
};
```

The environment variable makes one binary produce both sides of the
comparison. It is a prototype device, not the intended interface (section 6).

## 4. Non-regression over the library tests

### 4.1 Protocol

- **A**: `faust-rs` unchanged (no variable), faustlibraries as is, with the
  clamp.
- **B**: `FAUST_RS_MAX_SAMPLE_RATE=192000`, `platform.lib` without the clamp.
  That is the only library change.
- For each of the 1445 `*_test` definitions: compile with `--double` and
  faustlibraries' `arch/precision_arch.cpp`, then `c++ -O1`, and render
  0.25 s at 48, 192 and 384 kHz. The tests drive themselves (`no.noise`,
  `os.osc`...), and buttons are pressed.
- **Compared**: the compilation result, delay memory (the sum of the class's
  array sizes), and the outputs, bitwise, at each rate.

The script is in the appendix.

### 4.2 Results

| | A (clamp) | B (typing bound, no clamp) |
|---|---|---|
| tests that compile | 1445 | 1445 |
| delay memory | — | **identical for all 1445** |
| outputs at 48 kHz | — | **bitwise identical, 1445/1445** |
| outputs at 192 kHz | — | **bitwise identical, 1445/1445** |
| outputs at 384 kHz | — | 1088 differ (they depend on `SR`), 357 identical, none non-finite |

### 4.3 Above 192 kHz the result becomes right

`fi.lowpass(2, 1000)`, from its impulse response (1 s, FFT):

| rate | −3 dB, A | −3 dB, B | \|H(1 kHz)\| A | \|H(1 kHz)\| B |
|---|---|---|---|---|
| 48 kHz | 1000 Hz | 1000 Hz | −3.01 dB | −3.01 dB |
| 192 kHz | 1000 Hz | 1000 Hz | −3.01 dB | −3.01 dB |
| 384 kHz | **2000 Hz** | 1000 Hz | −0.26 dB | −3.01 dB |
| 768 kHz | **4000 Hz** | 1000 Hz | −0.02 dB | −3.01 dB |

Column A reproduces the table of faust#1321 exactly.

### 4.4 `upsampling`: right values, delays sized for `R * H`

For `process = (8, _) : upsampling(fi.lowpass(2, 1000) : de.delay(ma.SR, 0.01*ma.SR));`:

| | `SR` inside the domain | the 0.01 s delay line |
|---|---|---|
| A | `fmin(192000, fmax(1, fSampleRate*8))`, wrong | `float fRec0_4_d0[2048]`, sized for 192 kHz |
| B | `fmax(1, fSampleRate*8)`, right | `float fRec0_4_d0[16384]`, sized for 8 × 192 kHz |

### 4.5 Without the typing bound: the 42 delays sized from `SR`

With the clamp removed and **no** typing bound (variable unset), 42 of the 1445
tests fail with `[FRS-SFIR-0004] delay line size conversion overflow:
2147483648`. These are the library delays sized from `SR`, which the clamp was
silently bounding. They are the list to audit on the library side:

- `analyzers_tests.dsp`: `spectral_flux_test`
- `compressors_tests.dsp`: `compressor_lad_mono_test`, `limiter_lad_N_test`,
  `limiter_lad_bw_test`, `limiter_lad_mono_test`, `limiter_lad_quad_test`,
  `limiter_lad_stereo_test`
- `demos_tests.dsp`: `freeverb_demo_test`, `kb_rom_rev1_demo_test`,
  `reverbTank_demo_test`, `springreverb_demo_test`, `tapeStop_demo_test`,
  `vital_rev_demo_test`
- `physmodels_tests.dsp` (the waveguide strings and tubes): `fluteModel_test`,
  `fluteModel_ui_test`, `flute_ui_MIDI_test`, `flute_ui_test`,
  `idealString_test`, `ks_test`, `ks_ui_MIDI_test`, `marimbaModel_test`,
  `marimbaResTube_test`, `marimba_test`, `marimba_ui_MIDI_test`,
  `nylonString_test`, `openStringPickDown_test`, `openStringPickUp_test`,
  `openStringPick_test`, `openString_test`, `openTube_test`, `steelString_test`,
  `stringSegment_test`, `violinBowedString_test`, `violinModel_test`,
  `violin_ui_MIDI_test`, `violin_ui_test`
- `reverbs_tests.dsp`: `kb_rom_rev1_test`, `mono_freeverb_test`,
  `springreverb_test`, `stereo_freeverb_test`, `vital_rev_test`
- `spats_tests.dsp`: `wfs_ui_test`

This shows that the bound has to exist somewhere. It also shows that today's
error is not the one item 2 of the issue asks for: it names neither the delay
nor an option that would fix it.

## 5. Run-time safety when the host exceeds `R`

If `init()` is called above `R`, a delay sized from `SR` can ask for more
samples than were allocated, because the interval analysis assumed `R`. What
happens depends on the delay strategy (`crates/transform/src/signal_fir/delay/options.rs`):

- **`CircularPow2`** (the default above `max_copy_delay = 16`): reads are
  masked, `fVec[(fIOTA - N) & (S - 1)]`. The sample read is wrong, but memory
  is safe.
- **`Shift`** (delays up to 16 samples, `-mcd`): the history is indexed
  directly. It needs a clamp or a check.
- **`IfWrapping`** (`-dlt`, off by default): it computes `idx + size - N`, which
  becomes negative when `N > size`. It needs a clamp or a check.

Item 4 of the issue (clamp the read to the buffer, report the rate from
`init()`) is therefore needed for the last two strategies.

## 6. What remains

1. **A real option.** Replace the environment variable by
   `--max-sample-rate R` in `CompileOptionArgs`
   (`crates/compiler/src/compile_options.rs:44`), threaded to the type
   annotator, default 192000. It must reach every entry point that
   `CompileOptionArgs` serves: the CLI, faustprobe, the impulse runners, and
   the Cranelift/Interp/Wasm FFI.
2. **Unit tests in `sigtype`**, with the options, not with the environment:
   - `fSamplingFreq` is typed `[1, R]`, and the open interval without the
     option;
   - a delay `SR * 0.5` is sized `next_pow2(R/2)`;
   - in `upsampling(8)` with a constant clock, it is sized for `8R`;
   - a compact test-local `SR` definition, since tests must not depend on
     installed libraries.
3. **Structural non-regression.** Assert that the default generated code is
   unchanged, as this document measured over faustlibraries.
4. **Run-time safety** for the `Shift` and `IfWrapping` strategies (section 5),
   and a report from `init()` above `R`.
5. **A better diagnostic** for an unbounded delay (item 2 of the issue): name
   the delay, the size it needs, and the option that would bound it.
6. **The C++ compiler** needs the same change in `inferFConstType`
   (`sigtyperules.cpp`) **before** `platform.lib` loses its clamp. Otherwise the
   42 tests of section 4.5 stop compiling with it.
7. **Differential tests against C++** once the C++ side exists. Until then this
   is an `adapted` mapping (the typing departs from `inferFConstType`), to be
   recorded as such.
8. **Other backends.** The change is in typing, shared by all backends, but
   only the C++ output was measured here.
9. **Precision above 192 kHz, on the library side.** With a true `SR` at 384 or
   768 kHz, low-frequency poles come 2 to 4 times closer to z = 1 than at
   192 kHz. Direct forms lose even more in float. faustlibraries'
   `check_precision.py` stops at 192 kHz because `SR` was clamped, and would
   gain `--rates …,384000,768000`.

## Appendix: the comparison script

It needs `libsA/` and `libsB/`: two `git archive HEAD -- ':(glob)**/*.lib'
tests` extractions of faustlibraries, where `libsB/platform.lib` has its
`min(192000.0, …)` removed. Usage: `python3 nonreg.py <faust-rs> 192000
[REGEX]`.

```python
import concurrent.futures as cf
import json, os, re, subprocess, sys
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
FRS = sys.argv[1]
R = sys.argv[2] if len(sys.argv) > 2 else "192000"
ARCH = "/path/to/faustlibraries/arch/precision_arch.cpp"
SIDES = {"A": (os.path.join(HERE, "libsA"), {}),
         "B": (os.path.join(HERE, "libsB"), {"FAUST_RS_MAX_SAMPLE_RATE": R})}
RATES = [48000, 192000, 384000]
SECONDS = 0.25
BUILD = os.path.join(HERE, "build")
TEST_RE = re.compile(r"^\s*([A-Za-z0-9_]+_test)\s*=", re.M)
ARRAY_RE = re.compile(r"^\s+(?:float|double|int|FAUSTFLOAT)\s+\w+\[(\d+)\];", re.M)


def specs():
    out = []
    tdir = os.path.join(HERE, "libsA", "tests")
    for f in sorted(os.listdir(tdir)):
        if f.endswith(".dsp"):
            for n in TEST_RE.findall(open(os.path.join(tdir, f)).read()):
                out.append((f, n))
    return out


def build(side, f, name):
    libs, env = SIDES[side]
    d = os.path.join(BUILD, side)
    os.makedirs(d, exist_ok=True)
    cpp, exe = os.path.join(d, name + ".cpp"), os.path.join(d, name)
    r = subprocess.run([FRS, "--double", "-I", libs, "-a", ARCH, "-pn", name,
                        os.path.join(libs, "tests", f), "-o", cpp],
                       cwd=libs, env=dict(os.environ, **env), capture_output=True, text=True,
                       timeout=600)
    if r.returncode != 0:
        return None, None, "faust-rs: " + ((r.stderr or r.stdout).strip().splitlines() or ["?"])[-1][:200]
    text = open(cpp).read()
    cls = text[text.find("class mydsp"):]
    mem = sum(int(n) for n in ARRAY_RE.findall(cls))
    r = subprocess.run(["c++", "-O1", "-std=c++17", cpp, "-o", exe], capture_output=True, text=True)
    if r.returncode != 0:
        return None, mem, "c++: " + (r.stderr.strip().splitlines() or ["?"])[0][:200]
    return exe, mem, None


def render(exe, sr):
    frames = int(sr * SECONDS)
    out = exe + f".{sr}.raw"
    r = subprocess.run([exe, str(frames), str(sr), out], capture_output=True, timeout=600)
    if r.returncode != 0:
        return None
    with open(out, "rb") as fh:
        ch = int(np.frombuffer(fh.read(4), dtype=np.int32)[0])
        data = np.frombuffer(fh.read(), dtype=np.float64)
    os.remove(out)
    return data.reshape(-1, ch) if ch else np.zeros((frames, 0))


def check(spec):
    f, name = spec
    res = {"test": name, "file": f}
    exes = {}
    for side in "AB":
        exe, mem, err = build(side, f, name)
        res[f"mem{side}"] = mem
        if err:
            res[f"err{side}"] = err
        exes[side] = exe
    if not (exes["A"] and exes["B"]):
        return res
    for sr in RATES:
        a, b = render(exes["A"], sr), render(exes["B"], sr)
        if a is None or b is None or a.shape != b.shape:
            res[str(sr)] = "render error"
            continue
        peak = float(np.nanmax(np.abs(a))) if a.size else 0.0
        diff = float(np.nanmax(np.abs(a - b))) if a.size else 0.0
        res[str(sr)] = {"identical": bool(np.array_equal(a, b)),
                        "gap": diff / peak if peak > 0 else diff,
                        "finiteA": bool(np.isfinite(a).all()), "finiteB": bool(np.isfinite(b).all())}
    return res


if __name__ == "__main__":
    todo = specs()
    if len(sys.argv) > 3:
        todo = [s for s in todo if re.search(sys.argv[3], s[1])]
    results = []
    with cf.ThreadPoolExecutor(max_workers=os.cpu_count()) as pool:
        for i, r in enumerate(pool.map(check, todo)):
            results.append(r)
            if (i + 1) % 100 == 0:
                print(f"{i + 1}/{len(todo)}", flush=True)
    json.dump(results, open(os.path.join(HERE, f"nonreg_R{R}.json"), "w"), indent=1)
    print(f"{len(results)} tests done")
```

Section 4.5 comes from compiling the same 1445 tests on side B with
`FAUST_RS_MAX_SAMPLE_RATE` unset (`faust-rs --double -I libsB -pn NAME FILE -o
/dev/null`).
