// optimizers.lib: `gated(C)` with `stop_relative`: a clocked loop learning a
// gain, gated by the relative change of its period loss between checkpoints
// two periods apart. The target carries measurement noise, so the loss
// falls until the model error reaches the noise floor and flattens there;
// the criterion fires after at least four periods, and the gated gain holds
// from there while the plain one goes on.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [g_plain, g_gated, done]

op = library("optimizers.lib");

noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };
noise2 = lcg * 4.656612873077393e-10
with { lcg = +(54321) ~ *(1103515245); };
clock = ((+(1) : %(64)) ~ _) == 0;
x = noise;
target = 0.7 * x + 0.05 * noise2;

learn(xi, ti) = g, (loss(g) : op.stop_relative(clock, 2, 4, 40, 0.05))
with {
    loss(p) = op.mse(p * xi, ti);
    g = op.descend_1D_clocked(clock, loss, op.sgd_g(0.5), -4.0, 4.0, 0.0, 0.0);
};

plain = (x, target) : learn : _, !;
gg = (x, target) : op.gated(learn);

process = plain, gg;
