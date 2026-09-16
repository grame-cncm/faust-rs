// optimizers.lib: the plateau detector `stalled(a, eps_g, eps_l, g, l)` on
// three synthetic segments of 20 000 samples, (gradient, loss):
//   A  (0.5, 1.0)    descending: large gradient, large loss  -> 0
//   B  (0.0, 1.0)    stuck: no gradient, the loss still high  -> 1
//   C  (0.0, 0.001)  converged: no gradient, the loss at its floor -> 0
// with a = 0.999 (a 1 000-sample horizon), eps_g = 0.01, eps_l = 0.1. The
// flag settles within a few thousand samples of each segment start.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [stalled, g, l]

op = library("optimizers.lib");
ba = library("basics.lib");

seg = ba.time / 20000;
g = ba.selectn(3, seg, 0.5, 0.0, 0.0);
l = ba.selectn(3, seg, 1.0, 1.0, 0.001);

process = op.stalled(0.999, 0.01, 0.1, g, l), g, l;
