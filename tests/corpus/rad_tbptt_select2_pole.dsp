// TBPTT online identification of a one-pole with a `select2` in its
// feedback path.
//
// Target: y_target[n] = x[n] + half_wave(p_star * y_target[n-1])
// Model:  y_pred[n]   = x[n] + half_wave(p * y_pred[n-1])
//   with  half_wave(v) = select2(v > 0, 0.5 v, v): gain 1 on the positive
//   half, 0.5 on the negative half.
//
// The `select2` sits inside the recursive body, so the block reverse sweep
// has to route the adjoint to the branch that was taken at each sample: the
// condition is replayed from an integer tape. Before that rule existed the
// program was rejected with `FRS-SFIR-0004` on the `Select2` node.
//
// Update:      p[n+1] = clip(p[n] - lr * d(loss)/dp, -0.99, 0.99)
// Convergence: p[n] -> p_star; residual -> 0.
//
// Outputs: [residual_L, residual_R]

p_star = 0.7;
lr     = 0.005;

// Inline LCG white noise excitation
noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };

half_wave(v) = select2(v > 0.0, 0.5 * v, v);
nl_filter(p, x) = x : + ~ (*(p) : half_wave);

y_target = nl_filter(p_star, noise);

p_learned = loop ~ _
with {
    loop(p) = p_next
    with {
        y_pred = nl_filter(p, noise);
        loss   = (y_target - y_pred) * (y_target - y_pred);
        grad   = rad(loss, p) : !, _;
        p_next = max(-0.99, min(0.99, p - lr * grad));
    };
};

process = (y_target - nl_filter(p_learned, noise)) <: _, _;
