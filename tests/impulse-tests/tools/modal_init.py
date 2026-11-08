#!/usr/bin/env python3
"""Starting modes for the GPU modal fit of a recording.

`archs/modalfadmojogpubatch.mojo fit-file` refines a bank of modes against a
recording by `fad` and Gauss-Newton, but only from a start within about one
bandwidth of each mode (porting/faust-gpu-audio-synthesis-2026-11-07-en.md,
§5). This script gives that start, from the recording itself:

1. the strike: the steepest rise of the energy within the excerpt's first
   second (an earlier strike may still ring); the target is the recording
   from there, cut to a whole number of 1,024-sample blocks;
2. the frequencies: the peaks of the Hann-windowed, 4x zero-padded spectrum
   that stand at least `--prominence` dB above the lowest point within
   `--width` Hz on each side, at most `--floor` dB below the strongest, at
   least `--separation` Hz apart and above `--fmin` Hz, the strongest
   `--modes` kept; refined by a
   parabola through the log magnitudes;
3. the decays: the recording demodulated at each frequency and smoothed, its
   log magnitude fitted by a line over the part above the noise; the T60
   from the slope;
4. the amplitudes: with the frequencies and decays fixed, the sine and
   cosine amplitudes of every mode solved jointly by least squares, the Gram
   matrix of the damped sinusoids in closed form (geometric series), so that
   overlapping modes share the signal instead of each claiming it.

The model is the voice's, tests/corpus/gpu_modal_fad_voice_phase.dsp:
y[n] = r^n (gain sin(w n) + gainc cos(w n)), r = 0.001^(1 / (t60 SR)).

    modal_init.py <in.f32> <out-prefix> [--start s] [--seconds s] [--modes n]
                  [--floor dB] [--prominence dB] [--width Hz] [--separation Hz]
                  [--fmin Hz]

<in.f32> is raw native-endian mono f32 at 48 kHz (`sox in.wav -r 48000 -c 1
-t f32 out.f32`). Writes <out-prefix>.target.f32 and <out-prefix>.modes.txt.
Needs numpy (an opt-in lane tool, not used by CI).
"""

import argparse
import sys

import numpy as np

SR = 48000
BLOCK = 1024
LN1000 = np.log(1000.0)


def strike(x, start, seconds):
    seg = x[int(start * SR):int((start + seconds + 1.0) * SR)]
    # The strike: the steepest rise of the 1 ms energy within the first
    # second, minus 1 ms. A threshold on the level would fire on the ringing
    # of an earlier strike.
    ms = SR // 1000
    env = np.convolve(seg[: 2 * SR] ** 2, np.ones(ms) / ms, "same")
    rise = env[2 * ms:SR] - env[:SR - 2 * ms]
    on = max(0, int(np.argmax(rise)) + ms)
    n = (int(seconds * SR) // BLOCK) * BLOCK
    seg = seg[on:on + n]
    if len(seg) < n:
        sys.exit("modal_init.py: the excerpt is shorter than --seconds after the strike")
    return start + on / SR, seg


def peaks(seg, modes, floor, prominence, width, separation, fmin):
    n = len(seg)
    pad = 4 * n
    spec = np.fft.rfft(seg * np.hanning(n), pad)
    db = 20.0 * np.log10(np.abs(spec) + 1e-20)
    db -= db.max()
    hz = SR / pad
    w = max(1, int(width / hz))
    i = np.arange(1, len(db) - 1)
    cand = i[(db[i] > db[i - 1]) & (db[i] >= db[i + 1]) & (db[i] > -floor) & (i * hz >= fmin)]
    keep = []
    for k in cand:
        lo = db[max(0, k - w):k].min()
        hi = db[k + 1:k + 1 + w].min()
        if db[k] - max(lo, hi) >= prominence:
            keep.append(k)
    keep.sort(key=lambda k: -db[k])
    chosen = []
    for k in keep:
        if all(abs(k - c) * hz >= separation for c in chosen):
            chosen.append(k)
        if len(chosen) == modes:
            break
    chosen.sort()
    freqs = []
    for k in chosen:
        a, b, c = db[k - 1], db[k], db[k + 1]
        d = 0.5 * (a - c) / (a - 2 * b + c) if (a - 2 * b + c) != 0 else 0.0
        freqs.append((k + d) * hz)
    return np.array(freqs), np.array([db[k] for k in chosen])


def decays(seg, freqs):
    n = len(seg)
    t = np.arange(n)
    gaps = np.diff(freqs)
    t60s = []
    for k, f in enumerate(freqs):
        near = min(gaps[k - 1] if k > 0 else 1e9, gaps[k] if k < len(gaps) else 1e9)
        # A smoothing window short enough to keep the neighbours out.
        L = int(np.clip(SR / max(near, 1e-3), 480, 9600))
        z = seg * np.exp(-2j * np.pi * f * t / SR)
        c = np.cumsum(np.concatenate([[0.0], z]))
        m = np.abs(c[L:] - c[:-L]) / L
        hop = L // 4
        env = 20.0 * np.log10(m[::hop] + 1e-20)
        times = (np.arange(len(env)) * hop + L / 2) / SR
        top = env.max()
        floor = np.median(env[-max(4, len(env) // 8):])
        ok = (times > 0.02) & (env > max(floor + 10.0, top - 60.0))
        ok &= np.cumsum(~ok) == 0 if ok[0] else ok
        sel = np.where(ok)[0]
        if len(sel) < 4:
            t60s.append(1.0)
            continue
        slope = np.polyfit(times[sel], env[sel], 1)[0]
        t60s.append(float(np.clip(-60.0 / slope, 0.05, 60.0)) if slope < 0 else 60.0)
    return np.array(t60s)


def geometric(z, n):
    """Sum of z^k for k < n, elementwise."""
    out = np.empty_like(z)
    near = np.abs(1.0 - z) < 1e-12
    out[~near] = (1.0 - z[~near] ** n) / (1.0 - z[~near])
    out[near] = n
    return out


def amplitudes(seg, freqs, t60s):
    n = len(seg)
    m = len(freqs)
    w = 2 * np.pi * freqs / SR
    r = np.exp(-LN1000 / (t60s * SR))
    rr = np.outer(r, r)
    wm = w[:, None] - w[None, :]
    wp = w[:, None] + w[None, :]
    sm = geometric(rr * np.exp(1j * wm), n)
    sp = geometric(rr * np.exp(1j * wp), n)
    cm, cp, nm, np_ = sm.real, sp.real, sm.imag, sp.imag
    g = np.empty((2 * m, 2 * m))
    g[:m, :m] = 0.5 * (cm - cp)        # sin_k sin_l
    g[m:, m:] = 0.5 * (cm + cp)        # cos_k cos_l
    g[:m, m:] = 0.5 * (np_ + nm)       # sin_k cos_l
    g[m:, :m] = g[:m, m:].T
    rhs = np.empty(2 * m)
    t = np.arange(n)
    for k in range(m):
        z = (r[k] ** t) * np.exp(1j * w[k] * t)
        p = np.dot(seg, z)
        rhs[k] = p.imag
        rhs[m + k] = p.real
    ridge = 1e-9 * np.trace(g) / (2 * m)
    # numpy on Apple's Accelerate raises spurious floating-point flags in
    # matmul; the values are finite and checked below.
    with np.errstate(all="ignore"):
        x = np.linalg.solve(g + ridge * np.eye(2 * m), rhs)
        resid = float(np.dot(seg, seg) - 2.0 * np.dot(x, rhs) + x @ g @ x)
    if not np.all(np.isfinite(x)):
        sys.exit("modal_init.py: the amplitude solve failed (modes too close?)")
    return x[:m], x[m:], resid / float(np.dot(seg, seg))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("input")
    ap.add_argument("prefix")
    ap.add_argument("--start", type=float, default=0.0)
    ap.add_argument("--seconds", type=float, default=4.0)
    ap.add_argument("--modes", type=int, default=500)
    ap.add_argument("--floor", type=float, default=70.0)
    ap.add_argument("--prominence", type=float, default=10.0)
    ap.add_argument("--width", type=float, default=2.0)
    ap.add_argument("--separation", type=float, default=0.3)
    ap.add_argument("--fmin", type=float, default=50.0)
    a = ap.parse_args()
    x = np.fromfile(a.input, dtype=np.float32).astype(np.float64)
    t0, seg = strike(x, a.start, a.seconds)
    freqs, levels = peaks(seg, a.modes, a.floor, a.prominence, a.width, a.separation, a.fmin)
    t60s = decays(seg, freqs)
    gain, gainc, rel = amplitudes(seg, freqs, t60s)
    seg.astype(np.float32).tofile(a.prefix + ".target.f32")
    with open(a.prefix + ".modes.txt", "w") as f:
        f.write("# freq t60 gain gainc\n")
        for k in range(len(freqs)):
            f.write(f"{float(freqs[k])!r} {float(t60s[k])!r} {float(gain[k])!r} {float(gainc[k])!r}\n")
    print(
        f"strike at {t0:.4f} s, {len(seg)} samples ({len(seg) / SR:.3f} s), {len(freqs)} modes "
        f"from {freqs.min():.1f} to {freqs.max():.1f} Hz, T60 median {np.median(t60s):.2f} s; "
        f"least-squares start: residual {rel:.4e} of the energy ({-10 * np.log10(max(rel, 1e-30)):.1f} dB)"
    )


if __name__ == "__main__":
    main()
