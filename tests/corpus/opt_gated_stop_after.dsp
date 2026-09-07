// optimizers.lib: `gated(C)` around a clocked loop with a period budget. The
// block `learn` outputs the learned gain and `stop_after(clock, 5)`; gated,
// it runs on every sample until the fifth firing raises the flag, then
// computes nothing and holds. The same block outside the gate keeps
// learning, so the two gains must be bit-identical until the flag and the
// gated one constant afterwards.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [g_plain, g_gated, done, exp(g_gated) through on_change]

op = library("optimizers.lib");

noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };
clock = ((+(1) : %(64)) ~ _) == 0;
x = noise;

learn(xi) = g, op.stop_after(clock, 5)
with { g = op.descend_1D_clocked(clock, \(p).(op.mse(p * xi, 0.7 * xi)), op.sgd_g(0.5), -4.0, 4.0, 0.0, 0.0); };

plain = x : learn : _, !;
gg = x : op.gated(learn);
coef = (gg : _, !) : op.on_change(\(t).(exp(t)));

process = plain, gg, coef;
