// TBPTT identification of both poles in a 2-pole cascade IIR.
//
// Target: y_target[n] = (x : + ~ *(p1*)) : + ~ *(p2*)
//   i.e. two cascaded 1-pole recursive sections with hidden poles p1*, p2*.
//
// Model:  y_pred[n]   = (x : + ~ *(p1)) : + ~ *(p2)
//
// Both poles (p1, p2) are maintained in a 2-wire SYMREC and updated
// jointly each sample.  The BRA body contains two NESTED IIR recursions:
//
//   Stage 1: z[n]      = x[n] + p1 * z[n-1]
//   Stage 2: y_pred[n] = z[n] + p2 * y_pred[n-1]
//
// The backward sweep propagates through Stage 2 first, then Stage 1,
// each carrying a Delay1 adjoint across the SYMREC boundary.  This
// exercises the nested-IIR BRA path — two levels of `Delay1(SYMREF)`
// carry in the same sweep.
//
// Gradients (approximate, BS=1 direct term):
//   d(loss)/dp1 ≈ -2*e * (p2-filtered version of z[n-1])
//   d(loss)/dp2 ≈ -2*e * y_pred[n-1]
// where e = y_target - y_pred.
//
// Update:
//   p1[n+1] = clip(p1[n] - lr * dp1, -0.99, 0.99)
//   p2[n+1] = clip(p2[n] - lr * dp2, -0.99, 0.99)
//
// Convergence: (p1, p2) → (p1*, p2*); residual → 0.
//
// Outputs: [residual_L, residual_R]

p1_star = 0.6;    // hidden pole for stage 1
p2_star = 0.4;    // hidden pole for stage 2
lr      = 0.001;  // small lr — 2-pole IIR gradients can be large

// Inline LCG white noise
noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };

x        = noise;
y_target = x : + ~ *(p1_star) : + ~ *(p2_star);

// 2-wire SYMREC: (p1, p2) jointly updated each sample.
poles = loop ~ (_, _)
with {
    loop(p1, p2) = p1n, p2n
    with {
        // Two cascaded 1-pole sections (two nested IIR SYMRECs in BRA body)
        y_pred = x : + ~ *(p1) : + ~ *(p2);
        loss   = (y_target - y_pred) * (y_target - y_pred);
        g1     = rad(loss, p1) : !, _;   // BRA through both stages for p1
        g2     = rad(loss, p2) : !, _;   // BRA through stage 2 only for p2
        p1n    = max(-0.99, min(0.99, p1 - lr * g1));
        p2n    = max(-0.99, min(0.99, p2 - lr * g2));
    };
};

p1_l = poles : _, !;
p2_l = poles : !, _;

y_ia    = x : + ~ *(p1_l) : + ~ *(p2_l);
process = (y_target - y_ia) <: _, _;
