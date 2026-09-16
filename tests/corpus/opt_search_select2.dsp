// optimizers.lib: a discrete choice learned by the (1+1) evolution strategy.
//
// The model is `select2(p > 0.5, 0.2 x, 0.7 x)` and the target `0.7 x`: only
// the second branch matches, and the loss is a step in `p`. `descend_1D`
// never moves `p` from 0: the tangent through a comparison is zero.
// `search_1D_clocked` draws a candidate `p + u`, `u` uniform on [-1, 1], per
// 64-sample frame and keeps it when its frame loss is lower: within a few
// frames a candidate above 0.5 wins and the loss falls to zero.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [p by search, p by descend_1D, frame loss of the search]

import("stdfaust.lib");
op = library("optimizers.lib");

x = no.noise;
target = 0.7 * x;
mdl(p) = select2(p > 0.5, 0.2 * x, 0.7 * x);
loss(p) = op.mse(mdl(p), target);
clock = ((+(1) : %(64)) ~ _) == 0;

p_search = op.search_1D_clocked(clock, loss, 1.0, -2.0, 2.0, 0.0, 0.0);
p_descend = op.descend_1D(loss, op.sgd_g(0.5), -2.0, 2.0, 0.0, 0.0);

process = p_search, p_descend, op.frame_mean(clock, loss(p_search));
