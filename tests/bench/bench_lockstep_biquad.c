/*
 * bench_lockstep_biquad.c
 *
 * Micro-benchmark for "vertical" (lockstep) vectorization of 4 independent,
 * structurally identical recursive filters (RBJ peak biquads, DF2-transposed).
 *
 * Each biquad is serially recursive in time, so time-direction (-vec style)
 * vectorization is impossible. The question: how much is gained by running the
 * 4 instances in lockstep, one SIMD lane per instance?
 *
 * Versions:
 *   V0  one biquad, scalar loop                  (latency-bound reference)
 *   V1  4 biquads, 4 separate scalar loops       (current serial-loop lowering)
 *   V2  4 biquads, one fused time loop,
 *       4 explicit scalar chains, planar I/O     (plan-level fusion; compiler
 *                                                 free to ILP/SLP it)
 *   V3  fused loop, lane-innermost pure-C loop
 *       on interleaved buffers + boundary
 *       transposes per chunk                     (proposed FIR lowering,
 *                                                 auto-vectorized)
 *   V3k same kernel, I/O pre-interleaved,
 *       no transposes                            (interleaved-ABI scenario)
 *   V4  explicit SIMD (NEON/SSE) kernel +
 *       boundary transposes                      (manual ceiling)
 *   V4k explicit SIMD kernel, pre-interleaved    (ceiling, interleaved ABI)
 *
 * All versions perform the same IEEE op sequence per lane; bit-exactness vs V1
 * is checked and reported (FMA contraction differences are reported, not
 * hidden). No -ffast-math.
 *
 * Build: cc -O3 -std=c11 bench_lockstep_biquad.c -o bench -lm
 * Run:   ./bench [iters]
 */

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define LANES 4
#define NSAMP (1 << 16) /* samples per lane per iteration */
#define CHUNK 512       /* per-call block size, mimics compute() */
#define REPS 3          /* timed repetitions, best-of */
#define CHECKN 8192     /* samples for the bit-exactness pass */

typedef struct {
    float b0, b1, b2, a1, a2;
} Coefs;

/* SoA coefficient block for lane kernels */
typedef struct {
    float b0[LANES], b1[LANES], b2[LANES], a1[LANES], a2[LANES];
} Coefs4;

static Coefs rbj_peak(double fs, double f0, double q, double gdb)
{
    double A = pow(10.0, gdb / 40.0);
    double w0 = 2.0 * M_PI * f0 / fs;
    double alpha = sin(w0) / (2.0 * q);
    double a0 = 1.0 + alpha / A;
    Coefs c;
    c.b0 = (float)((1.0 + alpha * A) / a0);
    c.b1 = (float)((-2.0 * cos(w0)) / a0);
    c.b2 = (float)((1.0 - alpha * A) / a0);
    c.a1 = (float)((-2.0 * cos(w0)) / a0);
    c.a2 = (float)((1.0 - alpha / A) / a0);
    return c;
}

static uint32_t rng_state = 0x12345678u;
static float frand(void)
{
    rng_state ^= rng_state << 13;
    rng_state ^= rng_state >> 17;
    rng_state ^= rng_state << 5;
    return (float)(int32_t)rng_state * (0.5f / 2147483648.0f);
}

static double now_s(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + 1e-9 * (double)ts.tv_nsec;
}

static volatile double g_sink;

/* ---------- V0/V1: one scalar biquad (current serial-loop lowering) ------ */

__attribute__((noinline)) static void bq_scalar(int n, const float *restrict x,
                                                float *restrict y,
                                                const Coefs *restrict c,
                                                float *restrict st)
{
    float s1 = st[0], s2 = st[1];
    const float b0 = c->b0, b1 = c->b1, b2 = c->b2, a1 = c->a1, a2 = c->a2;
    for (int i = 0; i < n; i++) {
        float xi = x[i];
        float yi = b0 * xi + s1;
        s1 = b1 * xi - a1 * yi + s2;
        s2 = b2 * xi - a2 * yi;
        y[i] = yi;
    }
    st[0] = s1;
    st[1] = s2;
}

/* ---------- V2: one fused time loop, 4 explicit scalar chains ------------ */

__attribute__((noinline)) static void bq4_fused_scalar(
    int n, const float *restrict x0, const float *restrict x1,
    const float *restrict x2, const float *restrict x3, float *restrict y0,
    float *restrict y1, float *restrict y2, float *restrict y3,
    const Coefs4 *restrict c, float *restrict s1v, float *restrict s2v)
{
    float s10 = s1v[0], s11 = s1v[1], s12 = s1v[2], s13 = s1v[3];
    float s20 = s2v[0], s21 = s2v[1], s22 = s2v[2], s23 = s2v[3];
    for (int i = 0; i < n; i++) {
        float a, y;
        a = x0[i];
        y = c->b0[0] * a + s10;
        s10 = c->b1[0] * a - c->a1[0] * y + s20;
        s20 = c->b2[0] * a - c->a2[0] * y;
        y0[i] = y;

        a = x1[i];
        y = c->b0[1] * a + s11;
        s11 = c->b1[1] * a - c->a1[1] * y + s21;
        s21 = c->b2[1] * a - c->a2[1] * y;
        y1[i] = y;

        a = x2[i];
        y = c->b0[2] * a + s12;
        s12 = c->b1[2] * a - c->a1[2] * y + s22;
        s22 = c->b2[2] * a - c->a2[2] * y;
        y2[i] = y;

        a = x3[i];
        y = c->b0[3] * a + s13;
        s13 = c->b1[3] * a - c->a1[3] * y + s23;
        s23 = c->b2[3] * a - c->a2[3] * y;
        y3[i] = y;
    }
    s1v[0] = s10; s1v[1] = s11; s1v[2] = s12; s1v[3] = s13;
    s2v[0] = s20; s2v[1] = s21; s2v[2] = s22; s2v[3] = s23;
}

/* ---------- V3: lane-innermost pure C on interleaved buffers ------------- */

__attribute__((noinline)) static void bq4_lanes_c(int n,
                                                  const float *restrict xi,
                                                  float *restrict yo,
                                                  const Coefs4 *restrict c,
                                                  float *restrict s1v,
                                                  float *restrict s2v)
{
    float S1[LANES], S2[LANES];
    for (int l = 0; l < LANES; l++) {
        S1[l] = s1v[l];
        S2[l] = s2v[l];
    }
    for (int i = 0; i < n; i++) {
        for (int l = 0; l < LANES; l++) { /* innermost, no carried dep */
            float x = xi[LANES * i + l];
            float y = c->b0[l] * x + S1[l];
            S1[l] = c->b1[l] * x - c->a1[l] * y + S2[l];
            S2[l] = c->b2[l] * x - c->a2[l] * y;
            yo[LANES * i + l] = y;
        }
    }
    for (int l = 0; l < LANES; l++) {
        s1v[l] = S1[l];
        s2v[l] = S2[l];
    }
}

/* ---------- boundary transposes (layout-changing transports) ------------- */

__attribute__((noinline)) static void interleave4(int n, const float *restrict a,
                                                  const float *restrict b,
                                                  const float *restrict c,
                                                  const float *restrict d,
                                                  float *restrict out)
{
    for (int i = 0; i < n; i++) {
        out[4 * i + 0] = a[i];
        out[4 * i + 1] = b[i];
        out[4 * i + 2] = c[i];
        out[4 * i + 3] = d[i];
    }
}

__attribute__((noinline)) static void deinterleave4(int n,
                                                    const float *restrict in,
                                                    float *restrict a,
                                                    float *restrict b,
                                                    float *restrict c,
                                                    float *restrict d)
{
    for (int i = 0; i < n; i++) {
        a[i] = in[4 * i + 0];
        b[i] = in[4 * i + 1];
        c[i] = in[4 * i + 2];
        d[i] = in[4 * i + 3];
    }
}

/* ---------- V4: explicit SIMD kernel -------------------------------------- */

#if defined(__aarch64__) || defined(__ARM_NEON)
#include <arm_neon.h>
#define HAVE_SIMD 1
#define SIMD_NAME "NEON"

__attribute__((noinline)) static void bq4_simd(int n, const float *restrict xi,
                                               float *restrict yo,
                                               const Coefs4 *restrict c,
                                               float *restrict s1v,
                                               float *restrict s2v)
{
    float32x4_t B0 = vld1q_f32(c->b0), B1 = vld1q_f32(c->b1);
    float32x4_t B2 = vld1q_f32(c->b2);
    float32x4_t A1 = vld1q_f32(c->a1), A2 = vld1q_f32(c->a2);
    float32x4_t S1 = vld1q_f32(s1v), S2 = vld1q_f32(s2v);
    for (int i = 0; i < n; i++) {
        float32x4_t X = vld1q_f32(xi + 4 * i);
        float32x4_t Y = vfmaq_f32(S1, B0, X); /* b0*x + s1 */
        float32x4_t T = vfmsq_f32(vmulq_f32(B1, X), A1, Y); /* b1*x - a1*y */
        S1 = vaddq_f32(T, S2);
        S2 = vfmsq_f32(vmulq_f32(B2, X), A2, Y); /* b2*x - a2*y */
        vst1q_f32(yo + 4 * i, Y);
    }
    vst1q_f32(s1v, S1);
    vst1q_f32(s2v, S2);
}

#elif defined(__x86_64__)
#include <immintrin.h>
#define HAVE_SIMD 1
#ifdef __FMA__
#define SIMD_NAME "SSE+FMA"
#else
#define SIMD_NAME "SSE"
#endif

__attribute__((noinline)) static void bq4_simd(int n, const float *restrict xi,
                                               float *restrict yo,
                                               const Coefs4 *restrict c,
                                               float *restrict s1v,
                                               float *restrict s2v)
{
    __m128 B0 = _mm_loadu_ps(c->b0), B1 = _mm_loadu_ps(c->b1);
    __m128 B2 = _mm_loadu_ps(c->b2);
    __m128 A1 = _mm_loadu_ps(c->a1), A2 = _mm_loadu_ps(c->a2);
    __m128 S1 = _mm_loadu_ps(s1v), S2 = _mm_loadu_ps(s2v);
    for (int i = 0; i < n; i++) {
        __m128 X = _mm_loadu_ps(xi + 4 * i);
#ifdef __FMA__
        __m128 Y = _mm_fmadd_ps(B0, X, S1);
        __m128 T = _mm_fnmadd_ps(A1, Y, _mm_mul_ps(B1, X));
        S1 = _mm_add_ps(T, S2);
        S2 = _mm_fnmadd_ps(A2, Y, _mm_mul_ps(B2, X));
#else
        __m128 Y = _mm_add_ps(_mm_mul_ps(B0, X), S1);
        __m128 T = _mm_sub_ps(_mm_mul_ps(B1, X), _mm_mul_ps(A1, Y));
        S1 = _mm_add_ps(T, S2);
        S2 = _mm_sub_ps(_mm_mul_ps(B2, X), _mm_mul_ps(A2, Y));
#endif
        _mm_storeu_ps(yo + 4 * i, Y);
    }
    _mm_storeu_ps(s1v, S1);
    _mm_storeu_ps(s2v, S2);
}
#else
#define HAVE_SIMD 0
#endif

/* ---------- harness ------------------------------------------------------- */

typedef struct {
    const char *name;
    double best_s;      /* best-of-REPS wall time for ITERS iterations */
    int bitexact;       /* 1 if bit-identical to V1 on the check pass  */
    long mismatches;    /* number of differing samples in check pass   */
    float maxdiff;      /* max abs difference in check pass            */
} Result;

static float *falloc(size_t n)
{
    void *p = NULL;
    if (posix_memalign(&p, 64, n * sizeof(float)) != 0) {
        fprintf(stderr, "alloc failure\n");
        exit(1);
    }
    memset(p, 0, n * sizeof(float));
    return (float *)p;
}

static void compare(Result *r, const float *ref, const float *got, int n)
{
    long bad = 0;
    float md = 0.0f;
    for (int i = 0; i < n; i++) {
        if (memcmp(&ref[i], &got[i], 4) != 0) {
            bad++;
            float d = fabsf(ref[i] - got[i]);
            if (d > md)
                md = d;
        }
    }
    r->bitexact = (bad == 0);
    r->mismatches = bad;
    r->maxdiff = md;
}

int main(int argc, char **argv)
{
    long iters = (argc > 1) ? atol(argv[1]) : 800;
    printf("lockstep biquad micro-benchmark — %d lanes, %d samples/lane/iter, "
           "chunk %d, %ld iters, best of %d\n",
           LANES, NSAMP, CHUNK, iters, REPS);
#if HAVE_SIMD
    printf("explicit SIMD path: %s\n", SIMD_NAME);
#else
    printf("explicit SIMD path: none (V4 skipped)\n");
#endif

    Coefs c[LANES] = {
        rbj_peak(48000, 200, 0.8, 4.5),
        rbj_peak(48000, 1000, 1.2, -6.0),
        rbj_peak(48000, 3100, 2.0, 3.0),
        rbj_peak(48000, 8000, 0.7, -4.0),
    };
    Coefs4 c4;
    for (int l = 0; l < LANES; l++) {
        c4.b0[l] = c[l].b0;
        c4.b1[l] = c[l].b1;
        c4.b2[l] = c[l].b2;
        c4.a1[l] = c[l].a1;
        c4.a2[l] = c[l].a2;
    }

    float *in[LANES], *out[LANES];
    for (int l = 0; l < LANES; l++) {
        in[l] = falloc(NSAMP);
        out[l] = falloc(NSAMP);
        for (int i = 0; i < NSAMP; i++)
            in[l][i] = frand();
    }
    float *xi_full = falloc((size_t)NSAMP * LANES); /* pre-interleaved input  */
    float *yo_full = falloc((size_t)NSAMP * LANES); /* interleaved output     */
    float *xi_c = falloc((size_t)CHUNK * LANES);    /* chunk scratch          */
    float *yo_c = falloc((size_t)CHUNK * LANES);
    interleave4(NSAMP, in[0], in[1], in[2], in[3], xi_full);

    float st[LANES][2];
    float s1v[LANES], s2v[LANES];

    /* ---------- correctness pass (fresh state, CHECKN samples) ---------- */
    float *ref = falloc((size_t)CHECKN * LANES); /* reference, lane-major */
    float *chk = falloc((size_t)CHECKN * LANES);
    Result res[8];

    /* reference = V1 */
    for (int l = 0; l < LANES; l++) {
        st[l][0] = st[l][1] = 0.0f;
        bq_scalar(CHECKN, in[l], ref + (size_t)l * CHECKN, &c[l], st[l]);
    }

    /* V2 */
    {
        memset(s1v, 0, sizeof s1v);
        memset(s2v, 0, sizeof s2v);
        bq4_fused_scalar(CHECKN, in[0], in[1], in[2], in[3], chk,
                         chk + CHECKN, chk + 2 * CHECKN, chk + 3 * CHECKN, &c4,
                         s1v, s2v);
        res[1].name = "V2";
        compare(&res[1], ref, chk, CHECKN * LANES);
    }
    /* V3 kernel (on interleaved) */
    {
        memset(s1v, 0, sizeof s1v);
        memset(s2v, 0, sizeof s2v);
        bq4_lanes_c(CHECKN, xi_full, yo_full, &c4, s1v, s2v);
        deinterleave4(CHECKN, yo_full, chk, chk + CHECKN, chk + 2 * CHECKN,
                      chk + 3 * CHECKN);
        res[2].name = "V3";
        compare(&res[2], ref, chk, CHECKN * LANES);
    }
#if HAVE_SIMD
    /* V4 kernel */
    {
        memset(s1v, 0, sizeof s1v);
        memset(s2v, 0, sizeof s2v);
        bq4_simd(CHECKN, xi_full, yo_full, &c4, s1v, s2v);
        deinterleave4(CHECKN, yo_full, chk, chk + CHECKN, chk + 2 * CHECKN,
                      chk + 3 * CHECKN);
        res[3].name = "V4";
        compare(&res[3], ref, chk, CHECKN * LANES);
    }
#endif

    /* ---------- timed passes -------------------------------------------- */
    const int nchunks = NSAMP / CHUNK;
    double t0, t1;

#define TIME(varname, SETUP, BODY)                                            \
    do {                                                                      \
        double best = 1e30;                                                   \
        for (int rep = 0; rep < REPS; rep++) {                                \
            SETUP;                                                            \
            t0 = now_s();                                                     \
            for (long it = 0; it < iters; it++) {                             \
                for (int k = 0; k < nchunks; k++) {                           \
                    BODY;                                                     \
                }                                                             \
                g_sink += out[0][NSAMP - 1] + yo_full[4 * NSAMP - 1];         \
            }                                                                 \
            t1 = now_s();                                                     \
            if (t1 - t0 < best)                                               \
                best = t1 - t0;                                               \
        }                                                                     \
        varname = best;                                                       \
    } while (0)

    double tV0, tV1, tV2, tV3, tV3k, tV4 = 0, tV4k = 0;

    /* V0: one filter only */
    TIME(tV0, { st[0][0] = st[0][1] = 0.0f; }, {
        bq_scalar(CHUNK, in[0] + (size_t)k * CHUNK, out[0] + (size_t)k * CHUNK,
                  &c[0], st[0]);
    });

    /* V1: 4 separate scalar loops */
    TIME(tV1, { memset(st, 0, sizeof st); }, {
        for (int l = 0; l < LANES; l++)
            bq_scalar(CHUNK, in[l] + (size_t)k * CHUNK,
                      out[l] + (size_t)k * CHUNK, &c[l], st[l]);
    });

    /* V2: fused scalar */
    TIME(tV2,
         {
             memset(s1v, 0, sizeof s1v);
             memset(s2v, 0, sizeof s2v);
         },
         {
             size_t o = (size_t)k * CHUNK;
             bq4_fused_scalar(CHUNK, in[0] + o, in[1] + o, in[2] + o,
                              in[3] + o, out[0] + o, out[1] + o, out[2] + o,
                              out[3] + o, &c4, s1v, s2v);
         });

    /* V3: transpose + lane loop + detranspose, per chunk */
    TIME(tV3,
         {
             memset(s1v, 0, sizeof s1v);
             memset(s2v, 0, sizeof s2v);
         },
         {
             size_t o = (size_t)k * CHUNK;
             interleave4(CHUNK, in[0] + o, in[1] + o, in[2] + o, in[3] + o,
                         xi_c);
             bq4_lanes_c(CHUNK, xi_c, yo_c, &c4, s1v, s2v);
             deinterleave4(CHUNK, yo_c, out[0] + o, out[1] + o, out[2] + o,
                           out[3] + o);
         });

    /* V3k: lane loop only, pre-interleaved I/O */
    TIME(tV3k,
         {
             memset(s1v, 0, sizeof s1v);
             memset(s2v, 0, sizeof s2v);
         },
         {
             size_t o = (size_t)k * CHUNK * LANES;
             bq4_lanes_c(CHUNK, xi_full + o, yo_full + o, &c4, s1v, s2v);
         });

#if HAVE_SIMD
    /* V4: transposes + SIMD kernel */
    TIME(tV4,
         {
             memset(s1v, 0, sizeof s1v);
             memset(s2v, 0, sizeof s2v);
         },
         {
             size_t o = (size_t)k * CHUNK;
             interleave4(CHUNK, in[0] + o, in[1] + o, in[2] + o, in[3] + o,
                         xi_c);
             bq4_simd(CHUNK, xi_c, yo_c, &c4, s1v, s2v);
             deinterleave4(CHUNK, yo_c, out[0] + o, out[1] + o, out[2] + o,
                           out[3] + o);
         });

    /* V4k: SIMD kernel only, pre-interleaved I/O */
    TIME(tV4k,
         {
             memset(s1v, 0, sizeof s1v);
             memset(s2v, 0, sizeof s2v);
         },
         {
             size_t o = (size_t)k * CHUNK * LANES;
             bq4_simd(CHUNK, xi_full + o, yo_full + o, &c4, s1v, s2v);
         });
#endif

    /* ---------- report ---------------------------------------------------- */
    double lane_samples = (double)iters * NSAMP * LANES;
    double v0_lane_samples = (double)iters * NSAMP; /* V0 has one lane */

    printf("\n%-42s %10s %12s %9s  %s\n", "version", "time (s)",
           "ns/lane-sample", "vs V1", "bit-exact vs V1");
    printf("%-42s %10.3f %12.3f %9s  %s\n",
           "V0  single biquad, scalar", tV0, 1e9 * tV0 / v0_lane_samples, "-",
           "(reference lane)");
    printf("%-42s %10.3f %12.3f %8.2fx  %s\n",
           "V1  4 separate scalar loops", tV1, 1e9 * tV1 / lane_samples, 1.0,
           "(reference)");
#define ROW(label, t, r)                                                      \
    printf("%-42s %10.3f %12.3f %8.2fx  %s\n", label, t,                      \
           1e9 * (t) / lane_samples, tV1 / (t),                               \
           (r).bitexact ? "yes"                                               \
                        : "NO");                                              \
    if (!(r).bitexact)                                                        \
        printf("%-42s   mismatches=%ld maxdiff=%g\n", "", (r).mismatches,     \
               (r).maxdiff);

    ROW("V2  fused time loop, 4 scalar chains", tV2, res[1]);
    ROW("V3  lanes-inner C + chunk transposes", tV3, res[2]);
    { Result rk = res[2]; ROW("V3k lanes-inner C, interleaved ABI", tV3k, rk); }
#if HAVE_SIMD
    ROW("V4  explicit SIMD + chunk transposes", tV4, res[3]);
    { Result rk = res[3]; ROW("V4k explicit SIMD, interleaved ABI", tV4k, rk); }
    printf("\ntranspose overhead: V3 %+.1f%%  V4 %+.1f%%\n",
           100.0 * (tV3 / tV3k - 1.0), 100.0 * (tV4 / tV4k - 1.0));
#endif
    printf("(sink %g)\n", g_sink);
    return 0;
}
