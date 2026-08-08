// Self-contained compile-cost sentinel for the propagation shape isolated in
// ja_processor_stereo_ui and ja_transformer_demo. It deliberately keeps a
// recursively smoothed UI argument, four cascaded nonlinear recursive substeps,
// and stereo duplication. Do not replace the smoothing with a source-level
// workaround: the compiler must preserve and share this valid signal graph.

smooth(s) = smooth_imp
with {
    smooth_imp(x) = loop ~ _
    with {
        loop(y) = (1.0 - s) * x + s * y;
    };
};

tanh = ffunction(float tanhf|tanh|tanhl (float), <math.h>, "");

ja_core(Ms) = core
with {
    Ms_safe = max(Ms, 1e-6);
    a_norm = 720.0 / Ms_safe;
    k_norm = 380.0 / Ms_safe;
    inv_a_norm = 1.0 / max(a_norm, 1e-9);

    substep(M_prev, H_prev, H_target) = M_new
    with {
        dH = H_target - H_prev;
        He = H_target + 0.015 * M_prev;
        Man = tanh(He * inv_a_norm);
        dMan = (1.0 - Man * Man) * inv_a_norm;
        diff = Man - M_prev;
        diff_clamped = diff / (1.0 + abs(diff) * 3.0);
        pin = 380.0 * k_norm - 0.015 * diff_clamped;
        dMdH = (0.25 * dMan + diff_clamped / (pin + 1e-3))
             / (1.0 - 0.25 * 0.015 * dMan + 1e-9);
        M_new = max(-1.0, min(1.0, M_prev + dMdH * dH));
    };

    core(H_in) = loop ~ _
    with {
        loop(M_prev) = M4
        with {
            H_prev = H_in@1;
            H1 = 0.75 * H_prev + 0.25 * H_in;
            H2 = 0.50 * H_prev + 0.50 * H_in;
            H3 = 0.25 * H_prev + 0.75 * H_in;
            M1 = substep(M_prev, H_prev, H1);
            M2 = substep(M1, H1, H2);
            M3 = substep(M2, H2, H3);
            M4 = substep(M3, H3, H_in);
        };
    };
};

dc = _ <: _, @(1) : -;

// Three reduced cores retain the original absolute smoothed/raw delta while
// keeping this fixture independent of filters.lib.
processor(Ms) = ja_core(Ms) : ja_core(Ms) : ja_core(Ms) : dc;

ms_raw = hslider("Ms raw", 380, 100, 1000, 1);
ms_smooth = hslider("Ms smooth", 380, 100, 1000, 1) : smooth(0.999);

raw_mono = processor(ms_raw);
smooth_mono = processor(ms_smooth);
raw_stereo = par(i, 2, processor(ms_raw));
smooth_stereo = par(i, 2, processor(ms_smooth));

process = smooth_stereo;
