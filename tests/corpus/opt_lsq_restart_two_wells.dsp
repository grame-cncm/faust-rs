// optimizers.lib: `lsq_1D_restart` on a least-squares problem with a local
// minimum.
//
// The model is `x * L(p)` with `L(p) = (p^2 - 1)^2 + 0.3 p`, the two-well
// polynomial of opt_langevin_two_wells.dsp (a shallow well of 0.29 at 0.96,
// a deep one of -0.31 at -1.04), and the target is `-0.2 x`: the residual
// vanishes where `L(p) = -0.2`, on the slopes of the deep well, and never in
// the shallow one, where it settles at a power of 0.24 E[x^2], above
// eps_l = 0.05. NLMS from p = 1 stops in the shallow well; after 2 W = 8 000
// samples without progress the loop takes the second start, p = -1, in the
// deep well, and reaches a root of `L(p) = -0.2`, where no further restart
// fires.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [p, index of the current start, residual]

import("stdfaust.lib");
op = library("optimizers.lib");

x = no.noise;
L(p) = (p * p - 1.0) * (p * p - 1.0) + 0.3 * p;
mdl(p, s) = s * L(p);
target = -0.2 * x;
init(k) = select2(k, 1.0, -1.0);
learned = op.lsq_1D_restart(2, init, mdl, op.nlms(0.01, 0.000001, 0.99), -3.0, 3.0, 4000, 0.05, 0.05, 0.0, target, x);
p = learned : _, !;
k = learned : !, _;

process = p, k, mdl(p, x) - target;
