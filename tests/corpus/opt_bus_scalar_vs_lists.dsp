// optimizers.lib: the scalar form of a bus loop and its list form with every
// entry equal give the same parameters, sample for sample. `descend_N` and
// `descend_N_clocked` on a three-tap FIR, once with `-4, 4, 0` and one engine,
// once with `(-4, -4, -4)`, `(4, 4, 4)`, `(0, 0, 0)` and three copies of the
// engine. Outputs: the four residuals, scalar and list form of each loop; the
// test asks that each pair be identical.
import("stdfaust.lib");
op = library("optimizers.lib");

x = no.noise;
target = 0.5 * x + 0.3 * x' - 0.2 * x'';
loss(a, b, c) = op.mse(a * x + b * x' + c * x'', target);
resid(P) = (P : \(a, b, c).(a * x + b * x' + c * x'')) - target;
clock = ((+(1) : %(64)) ~ _) == 0;
adam = op.adam_g(0.01, 0.9, 0.999, 1e-8);
sgd = op.sgd_g(0.5);
process = resid(op.descend_N(3, loss, adam, -4, 4, 0, 0)),
          resid(op.descend_N(3, loss, (adam, adam, adam), (-4, -4, -4), (4, 4, 4), (0, 0, 0), 0)),
          resid(op.descend_N_clocked(3, clock, loss, sgd, -4, 4, 0, 0)),
          resid(op.descend_N_clocked(3, clock, loss, (sgd, sgd, sgd), (-4, -4, -4), (4, 4, 4), (0, 0, 0), 0));
