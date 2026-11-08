// ---------------------------------------------------------------------------
// modalfadcpufile.cpp — the CPU baseline of `fit-file` in
// archs/modalfadmojogpubatch.mojo (NPARAM=4): the same fit of a recording,
// with the same target, the same starting modes, and the same Adam update in
// the same coordinates, as a CPU user would write it. It uses ordinary
// instances of the C++ programs faust-rs generates from
// tests/corpus/gpu_modal_fad_group_phase.dsp (four modes per program),
// compiled by clang and run on std::thread.
//
// Built by concatenation:
//   1. archs/modalfadcpu.cpp without MODALFAD_DRIVER: the minimal
//      `dsp`/`UI`/`Meta` interface;
//   2. `plainvoice` (`-pn plain`) and `fadvoice`;
//   3. this file.
//
//   <binary> <target.f32> <modes.txt> <epochs> [<threads>...]
//
// One epoch, as in archs/modalfadcpu.cpp:
//   1. the plain modes give e = target - mix;
//   2. the fad program reduces its four tangents per mode against e;
//   3. Adam steps every mode (cosine-decayed rate from 0.05, as `fit-file
//      ... adam 0.05`).
// It prints, per thread count: the median epoch time, the loss at the start
// and the sum of the gradient magnitudes of the first epoch (to compare with
// the GPU's), and the loss of the last pass.
// ---------------------------------------------------------------------------

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

static const int RATE = 48000;
static const int CHUNK = 512;
static const int BLOCK = 1024;
static const int G = 4;   // modes per program
static const int NP = 4;  // freq, t60, gain, gainc
static const double LN1000 = 6.907755278982137;

template <class D>
struct Voice {
    D dsp;
    UI ui;
    float* zone[NP][G];
    void init()
    {
        dsp.init(RATE);
        dsp.buildUserInterface(&ui);
        const char* names[NP] = {"freq", "t60", "gain", "gainc"};
        for (int p = 0; p < NP; p++) {
            for (int j = 0; j < G; j++) zone[p][j] = ui.zones.at(names[p] + std::to_string(j));
        }
    }
    // Modes k0 .. k0 + G - 1; past n, silent.
    void reset(const std::vector<float>& par, int n, int k0)
    {
        dsp.instanceClear();
        const float idle[NP] = {440.f, 1.f, 0.f, 0.f};
        for (int j = 0; j < G; j++) {
            int k = k0 + j;
            for (int p = 0; p < NP; p++) *zone[p][j] = k < n ? par[p * n + k] : idle[p];
        }
    }
};

static std::vector<float> read_f32(const char* path)
{
    std::ifstream f(path, std::ios::binary | std::ios::ate);
    std::vector<float> x(size_t(f.tellg()) / 4);
    f.seekg(0);
    f.read(reinterpret_cast<char*>(x.data()), std::streamsize(x.size() * 4));
    return x;
}

int main(int argc, char** argv)
{
    if (argc < 4) {
        std::fprintf(stderr, "usage: <target.f32> <modes.txt> <epochs> [<threads>...]\n");
        return 2;
    }
    std::vector<float> tg = read_f32(argv[1]);
    int T = int(tg.size()) / BLOCK * BLOCK;
    tg.resize(T);
    std::vector<double> m0[NP];
    std::ifstream mf(argv[2]);
    for (std::string line; std::getline(mf, line);) {
        if (line.empty() || line[0] == '#') continue;
        std::istringstream is(line);
        double v[NP] = {0, 0, 0, 0};
        for (int p = 0; p < NP; p++) is >> v[p];
        for (int p = 0; p < NP; p++) m0[p].push_back(v[p]);
    }
    int n = int(m0[0].size()), groups = (n + G - 1) / G, epochs = std::atoi(argv[3]);
    std::vector<int> counts;
    for (int i = 4; i < argc; i++) counts.push_back(std::atoi(argv[i]));
    if (counts.empty()) counts = {1, 4, int(std::thread::hardware_concurrency())};
    double energy = 0;
    for (float x : tg) energy += double(x) * x;

    for (int threads : counts) {
        std::vector<Voice<plainvoice>> plain(groups);
        std::vector<Voice<fadvoice>> fad(groups);
        for (int q = 0; q < groups; q++) {
            plain[q].init();
            fad[q].init();
        }
        // Parameters, theta = (freq / bw0, log t60, gain / a0, gainc / a0),
        // scales (bw0, a0), Adam moments, gradient sums, residual.
        std::vector<float> par(NP * n), th(NP * n), m1(NP * n, 0.f), m2(NP * n, 0.f), sc(2 * n), acc(NP * n), e(T);
        std::vector<std::vector<float>> mix(threads, std::vector<float>(T));
        for (int k = 0; k < n; k++) {
            double bw = LN1000 / (M_PI * m0[1][k]);
            double a0 = std::max(std::sqrt(m0[2][k] * m0[2][k] + m0[3][k] * m0[3][k]), 1e-9);
            for (int p = 0; p < NP; p++) par[p * n + k] = float(m0[p][k]);
            sc[k] = float(bw);
            sc[n + k] = float(a0);
            th[k] = float(m0[0][k] / bw);
            th[n + k] = float(std::log(m0[1][k]));
            th[2 * n + k] = float(m0[2][k] / a0);
            th[3 * n + k] = float(m0[3][k] / a0);
        }
        auto parallel = [&](auto fn) {
            if (threads == 1) {
                fn(0);
                return;
            }
            std::vector<std::thread> ts;
            for (int w = 0; w < threads; w++) ts.emplace_back(fn, w);
            for (auto& t : ts) t.join();
        };
        std::vector<double> times;
        double l0 = 0, gabs = 0;
        for (int ep = 0; ep < epochs; ep++) {
            auto t0 = std::chrono::steady_clock::now();
            // 1. e = target - sum of the plain modes.
            parallel([&](int w) {
                float* mx = mix[w].data();
                std::fill(mx, mx + T, 0.f);
                float buf[CHUNK];
                float* outs[1] = {buf};
                for (int q = groups * w / threads; q < groups * (w + 1) / threads; q++) {
                    plain[q].reset(par, n, q * G);
                    for (int c = 0; c < T; c += CHUNK) {
                        plain[q].dsp.compute(CHUNK, nullptr, outs);
                        for (int i = 0; i < CHUNK; i++) mx[c + i] += buf[i];
                    }
                }
            });
            parallel([&](int w) {
                for (int i = T * w / threads; i < T * (w + 1) / threads; i++) {
                    float s = 0.f;
                    for (int t = 0; t < threads; t++) s += mix[t][i];
                    e[i] = tg[i] - s;
                }
            });
            // 2. Each mode's tangents against e.
            parallel([&](int w) {
                std::vector<float> buf((NP + 1) * G * CHUNK);
                float* outs[(NP + 1) * G];
                for (int c = 0; c < (NP + 1) * G; c++) outs[c] = buf.data() + c * CHUNK;
                for (int q = groups * w / threads; q < groups * (w + 1) / threads; q++) {
                    fad[q].reset(par, n, q * G);
                    float a[NP * G] = {0};
                    for (int c = 0; c < T; c += CHUNK) {
                        fad[q].dsp.compute(CHUNK, nullptr, outs);
                        const float* ee = e.data() + c;
                        for (int j = 0; j < G; j++) {
                            for (int p = 0; p < NP; p++) {
                                const float* d = outs[(NP + 1) * j + 1 + p];
                                float s = 0.f;
                                for (int i = 0; i < CHUNK; i++) s += ee[i] * d[i];
                                a[NP * j + p] += s;
                            }
                        }
                    }
                    for (int j = 0; j < G && q * G + j < n; j++) {
                        for (int p = 0; p < NP; p++) acc[p * n + q * G + j] = a[NP * j + p];
                    }
                }
            });
            if (ep == 0) {
                for (float x : e) l0 += double(x) * x;
                l0 /= energy;
                for (float a : acc) gabs += std::fabs(a);
            }
            // 3. Adam, as the GPU's adam_kernel.
            float lr = 0.05f * float(0.01 + 0.99 * 0.5 * (1 + std::cos(M_PI * ep / std::max(epochs - 1, 1))));
            float bc1 = 1.f - std::pow(0.9f, float(ep + 1)), bc2 = 1.f - std::pow(0.999f, float(ep + 1));
            float gs = float(-2.0 / energy);
            for (int k = 0; k < n; k++) {
                for (int p = 0; p < NP; p++) {
                    float c = p == 0 ? sc[k] : p == 1 ? par[n + k] : sc[n + k];
                    int i = p * n + k;
                    float g = gs * c * acc[i];
                    m1[i] = 0.9f * m1[i] + 0.1f * g;
                    m2[i] = 0.999f * m2[i] + 0.001f * g * g;
                    th[i] -= lr * (m1[i] / bc1) / (std::sqrt(m2[i] / bc2) + 1e-20f);
                }
                float f = std::max(20.f, std::min(0.45f * RATE, th[k] * sc[k]));
                float lt = std::max(-4.6f, std::min(3.4f, th[n + k]));
                th[k] = f / sc[k];
                th[n + k] = lt;
                par[k] = f;
                par[n + k] = std::exp(lt);
                for (int p = 2; p < NP; p++) par[p * n + k] = th[p * n + k] * sc[n + k];
            }
            times.push_back(std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count());
        }
        std::vector<double> warm(times.begin() + std::min<size_t>(2, times.size() - 1), times.end());
        std::sort(warm.begin(), warm.end());
        double loss = 0;
        for (float x : e) loss += double(x) * x;
        loss /= energy;
        std::printf(
            "modes %d frames %d threads %d epoch_ms %.2f loss_at_start %.6f grad_abs_sum %.6e last_pass_loss %.6f\n",
            n, T, threads, warm[warm.size() / 2], l0, gabs, loss);
    }
    return 0;
}
