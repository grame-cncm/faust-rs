// ---------------------------------------------------------------------------
// modalfadcpu.cpp — the CPU baseline of archs/modalfadmojogpubatch.mojo: the
// same modal fit (same hidden bank, same start, same loss and gradient), as a
// CPU user would write it, with ordinary instances of the C++ programs that
// faust-rs generates from tests/corpus/gpu_modal_fad_voice.dsp, compiled by
// clang (LLVM) and run on std::thread.
//
// Built by concatenation: a minimal `dsp`/`UI`/`Meta` interface (below, no
// installed Faust header), then `plainvoice` (`-pn plain`: the modes without
// tangents, summed) and `fadvoice` (`process`: each mode's y and its three
// tangents), both `-lang cpp -single`, then this file's driver
// (MODALFAD_DRIVER). The Makefile compiles tests/corpus/gpu_modal_fad_group.dsp
// (four modes per program, MODALFAD_GROUP 4), the fastest grouping measured;
// gpu_modal_fad_voice.dsp (MODALFAD_GROUP 1) is the one-mode form.
//
//   <binary> bench <modes> [<threads>...]      time per epoch, per thread count
//
// One epoch, two passes, each over every program instance split across the
// threads:
//   1. `plainvoice`, chunk by chunk, added into a per-thread mix; the mixes
//      summed and e = target - mix;
//   2. `fadvoice`, chunk by chunk, each mode's three tangents reduced
//      against e into its gradient sums;
// then the Adam step of every mode (the GPU's update, same coordinates).
// The CPU has every e before pass 2, so it needs no per-block lock-step and
// computes the cheap forward without tangents; the GPU computes y and the
// tangents once, in one pass. Both are the natural form on their device.
//
// Prints the loss at the start and a checksum of the gradient sums, which
// the GPU harness prints too (`bench`), as a cross-check that both do the
// same work.
// ---------------------------------------------------------------------------

#ifndef MODALFAD_DRIVER

#include <map>
#include <string>

#define FAUSTFLOAT float

struct Meta {
    virtual ~Meta() {}
    virtual void declare(const char*, const char*) {}
};

struct Soundfile;

struct UI {
    virtual ~UI() {}
    virtual void openTabBox(const char*) {}
    virtual void openHorizontalBox(const char*) {}
    virtual void openVerticalBox(const char*) {}
    virtual void closeBox() {}
    virtual void addButton(const char*, FAUSTFLOAT*) {}
    virtual void addCheckButton(const char*, FAUSTFLOAT*) {}
    virtual void addVerticalSlider(const char* l, FAUSTFLOAT* z, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT)
    {
        zones[l] = z;
    }
    virtual void addHorizontalSlider(const char* l, FAUSTFLOAT* z, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT)
    {
        zones[l] = z;
    }
    virtual void addNumEntry(const char* l, FAUSTFLOAT* z, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT)
    {
        zones[l] = z;
    }
    virtual void addHorizontalBargraph(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT) {}
    virtual void addVerticalBargraph(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT) {}
    virtual void addSoundfile(const char*, const char*, Soundfile**) {}
    virtual void declare(FAUSTFLOAT*, const char*, const char*) {}
    std::map<std::string, FAUSTFLOAT*> zones;
};

struct dsp {
    virtual ~dsp() {}
};

#else  // MODALFAD_DRIVER

#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <thread>
#include <vector>
#include <algorithm>

static const int RATE = 48000;
static const int SIGNAL_FRAMES = 49152;
static const int CHUNK = 512;
static const double LN1000 = 6.907755278982137;
static const double TWO_PI = 6.283185307179586;
static const double FLO = 100.0;
static const double FHI = 16000.0;

// The same hash, bank and start as the GPU harness.
static double hash01(int64_t a, int64_t b)
{
    uint64_t x = uint64_t(a) * 0x9E3779B97F4A7C15ull ^ uint64_t(b + 1) * 0xC2B2AE3D27D4EB4Full;
    x ^= x >> 31;
    x *= 0xBF58476D1CE4E5B9ull;
    x ^= x >> 29;
    return double(x >> 11) / 9007199254740992.0;
}

static double gauss(int a, int b)
{
    double u1 = std::max(hash01(a, b), 1e-12);
    double u2 = hash01(a, b + 1);
    return std::sqrt(-2.0 * std::log(u1)) * std::cos(TWO_PI * u2);
}

static double bandwidth(double t60) { return LN1000 / (M_PI * t60); }

struct Bank {
    std::vector<double> freq, t60, gain;
};

static Bank make_bank(int n)
{
    Bank b;
    double step = (FHI - FLO) / n;
    double level = 1.0 / std::sqrt(double(n));
    for (int k = 0; k < n; k++) {
        double f = FLO + step * (k + 0.5 + 0.8 * (hash01(k, 1) - 0.5));
        double t = 2.5 * std::pow(f / FLO, -0.35) * std::exp(0.6 * (hash01(k, 2) - 0.5));
        double g = level * std::pow(10.0, -24.0 * hash01(k, 3) / 20.0);
        if (hash01(k, 4) < 0.5) g = -g;
        b.freq.push_back(f);
        b.t60.push_back(t);
        b.gain.push_back(g);
    }
    return b;
}

static Bank perturbed(const Bank& h, double sigma)
{
    Bank b;
    for (size_t k = 0; k < h.freq.size(); k++) {
        b.freq.push_back(h.freq[k] + sigma * bandwidth(h.t60[k]) * gauss(int(k), 10));
        b.t60.push_back(h.t60[k] * std::exp(0.3 * sigma * gauss(int(k), 20)));
        b.gain.push_back(h.gain[k] * (1.0 + 0.3 * sigma * gauss(int(k), 30)));
    }
    return b;
}

// G modes per program instance (MODALFAD_GROUP, 1 for the one-mode
// programs): controls `freq`, `t60`, `gain`, or `freq0` ... `gain<G-1>`.
#ifndef MODALFAD_GROUP
#define MODALFAD_GROUP 1
#endif
static const int G = MODALFAD_GROUP;

template <class D>
struct Voice {
    D dsp;
    UI ui;
    float *freq[G], *t60[G], *gain[G];
    void init()
    {
        dsp.init(RATE);
        dsp.buildUserInterface(&ui);
        for (int j = 0; j < G; j++) {
            std::string s = G == 1 ? std::string() : std::to_string(j);
            freq[j] = ui.zones.at("freq" + s);
            t60[j] = ui.zones.at("t60" + s);
            gain[j] = ui.zones.at("gain" + s);
        }
    }
    // Modes k0 .. k0 + G - 1; past n, silent.
    void reset(const std::vector<float>& params, int n, int k0)
    {
        dsp.instanceClear();
        for (int j = 0; j < G; j++) {
            int k = k0 + j;
            *freq[j] = k < n ? params[k] : 440.f;
            *t60[j] = k < n ? params[n + k] : 1.f;
            *gain[j] = k < n ? params[2 * n + k] : 0.f;
        }
    }
};

struct Fit {
    int n, threads;
    std::vector<Voice<plainvoice>> plain;
    std::vector<Voice<fadvoice>> fad;
    std::vector<float> params;   // freq, t60, gain (3n)
    std::vector<float> theta, m1, m2, scale;
    std::vector<float> acc;      // 3n gradient sums
    std::vector<float> target, e;
    std::vector<std::vector<float>> mixes;
    double energy = 0.0;

    int groups;

    Fit(int n_, int threads_)
        : n(n_), threads(threads_), plain((n_ + G - 1) / G), fad((n_ + G - 1) / G), groups((n_ + G - 1) / G)
    {
        for (int k = 0; k < groups; k++) {
            plain[k].init();
            fad[k].init();
        }
        params.assign(3 * n, 0.f);
        acc.assign(3 * n, 0.f);
        target.assign(SIGNAL_FRAMES, 0.f);
        e.assign(SIGNAL_FRAMES, 0.f);
        mixes.assign(threads, std::vector<float>(SIGNAL_FRAMES, 0.f));
    }

    void set(const Bank& b)
    {
        for (int k = 0; k < n; k++) {
            params[k] = float(b.freq[k]);
            params[n + k] = float(b.t60[k]);
            params[2 * n + k] = float(b.gain[k]);
        }
    }

    template <class F>
    void parallel(F f)
    {
        if (threads == 1) {
            f(0);
            return;
        }
        std::vector<std::thread> ts;
        for (int w = 0; w < threads; w++) ts.emplace_back(f, w);
        for (auto& t : ts) t.join();
    }

    // Pass 1: e = target - sum of the plain modes.
    void forward()
    {
        parallel([this](int w) {
            float* mix = mixes[w].data();
            std::fill(mix, mix + SIGNAL_FRAMES, 0.f);
            float buf[CHUNK];
            float* outs[1] = {buf};
            for (int q = groups * w / threads; q < groups * (w + 1) / threads; q++) {
                plain[q].reset(params, n, q * G);
                for (int c = 0; c < SIGNAL_FRAMES; c += CHUNK) {
                    plain[q].dsp.compute(CHUNK, nullptr, outs);
                    for (int i = 0; i < CHUNK; i++) mix[c + i] += buf[i];
                }
            }
        });
        parallel([this](int w) {
            for (int i = SIGNAL_FRAMES * w / threads; i < SIGNAL_FRAMES * (w + 1) / threads; i++) {
                float s = 0.f;
                for (int t = 0; t < threads; t++) s += mixes[t][i];
                e[i] = target[i] - s;
            }
        });
    }

    // Pass 2: each mode's tangents against e.
    void gradient()
    {
        parallel([this](int w) {
            std::vector<float> buf(4 * G * CHUNK);
            float* outs[4 * G];
            for (int c = 0; c < 4 * G; c++) outs[c] = buf.data() + c * CHUNK;
            for (int q = groups * w / threads; q < groups * (w + 1) / threads; q++) {
                fad[q].reset(params, n, q * G);
                float a[3 * G];
                std::fill(a, a + 3 * G, 0.f);
                for (int c = 0; c < SIGNAL_FRAMES; c += CHUNK) {
                    fad[q].dsp.compute(CHUNK, nullptr, outs);
                    const float* ee = e.data() + c;
                    for (int j = 0; j < G; j++) {
                        const float* d0 = outs[4 * j + 1];
                        const float* d1 = outs[4 * j + 2];
                        const float* d2 = outs[4 * j + 3];
                        float a0 = 0.f, a1 = 0.f, a2 = 0.f;
                        for (int i = 0; i < CHUNK; i++) {
                            a0 += ee[i] * d0[i];
                            a1 += ee[i] * d1[i];
                            a2 += ee[i] * d2[i];
                        }
                        a[3 * j] += a0;
                        a[3 * j + 1] += a1;
                        a[3 * j + 2] += a2;
                    }
                }
                for (int j = 0; j < G && q * G + j < n; j++) {
                    int k = q * G + j;
                    acc[k] = a[3 * j];
                    acc[n + k] = a[3 * j + 1];
                    acc[2 * n + k] = a[3 * j + 2];
                }
            }
        });
    }

    void adam(int step, float lr)
    {
        const float b1 = 0.9f, b2 = 0.999f;
        float bc1 = 1.f - std::pow(b1, float(step)), bc2 = 1.f - std::pow(b2, float(step));
        float gscale = float(-2.0 / energy);
        for (int k = 0; k < n; k++) {
            float c[3] = {scale[k], params[n + k], scale[n + k]};
            for (int j = 0; j < 3; j++) {
                int i = j * n + k;
                float g = gscale * c[j] * acc[i];
                m1[i] = b1 * m1[i] + (1.f - b1) * g;
                m2[i] = b2 * m2[i] + (1.f - b2) * g * g;
                theta[i] -= lr * (m1[i] / bc1) / (std::sqrt(m2[i] / bc2) + 1e-20f);
            }
            float f = std::max(20.f, std::min(0.45f * RATE, theta[k] * scale[k]));
            float lt = std::max(-4.6f, std::min(3.4f, theta[n + k]));
            theta[k] = f / scale[k];
            theta[n + k] = lt;
            params[k] = f;
            params[n + k] = std::exp(lt);
            params[2 * n + k] = theta[2 * n + k] * scale[n + k];
        }
    }

    double loss() const
    {
        double s = 0.0;
        for (float x : e) s += double(x) * x;
        return s / energy;
    }
};

static double now_ms()
{
    return std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now().time_since_epoch()).count();
}

int main(int argc, char** argv)
{
    if (argc < 3 || std::strcmp(argv[1], "bench") != 0) {
        std::fprintf(stderr, "usage: bench <modes> [<threads>...]\n");
        return 2;
    }
    int n = std::atoi(argv[2]);
    std::vector<int> counts;
    for (int i = 3; i < argc; i++) counts.push_back(std::atoi(argv[i]));
    if (counts.empty()) counts = {1, 4, int(std::thread::hardware_concurrency())};
    Bank hidden = make_bank(n);
    Bank start = perturbed(hidden, 1.0);
    for (int threads : counts) {
        Fit fit(n, threads);
        // The target: the hidden bank, from the plain voice.
        fit.set(hidden);
        fit.forward();
        fit.energy = 0.0;
        for (int i = 0; i < SIGNAL_FRAMES; i++) {
            fit.target[i] = -fit.e[i];
            fit.energy += double(fit.e[i]) * fit.e[i];
        }
        fit.set(start);
        fit.theta.assign(3 * n, 0.f);
        fit.scale.assign(2 * n, 0.f);
        fit.m1.assign(3 * n, 0.f);
        fit.m2.assign(3 * n, 0.f);
        for (int k = 0; k < n; k++) {
            double sf = bandwidth(start.t60[k]), sg = std::max(std::fabs(start.gain[k]), 1e-9);
            fit.scale[k] = float(sf);
            fit.scale[n + k] = float(sg);
            fit.theta[k] = float(start.freq[k] / sf);
            fit.theta[n + k] = float(std::log(start.t60[k]));
            fit.theta[2 * n + k] = float(start.gain[k] / sg);
        }
        std::vector<double> times;
        double l0 = 0.0, checksum = 0.0;
        int epochs = n >= 8192 ? 8 : 13;
        for (int ep = 0; ep < epochs; ep++) {
            double t0 = now_ms();
            fit.forward();
            fit.gradient();
            if (ep == 0) {
                l0 = fit.loss();
                for (int i = 0; i < 3 * n; i++) checksum += std::fabs(double(fit.acc[i]));
            }
            fit.adam(ep + 1, 0.02f);
            times.push_back(now_ms() - t0);
        }
        std::vector<double> warm(times.begin() + 3, times.end());
        std::sort(warm.begin(), warm.end());
        double med = warm[warm.size() / 2];
        std::printf("modes %d group %d frames %d threads %d epoch_ms %.3f ns_per_mode_sample %.4f loss_at_start %.6f grad_abs_sum %.6e\n",
                    n, G, SIGNAL_FRAMES, threads, med, med * 1e6 / (double(n) * SIGNAL_FRAMES), l0, checksum);
    }
    return 0;
}

#endif  // MODALFAD_DRIVER
