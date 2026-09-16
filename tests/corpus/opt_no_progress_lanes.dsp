// optimizers.lib: the progress detector `no_progress(W, rel, eps_l, l)` on
// three synthetic segments of 20 000 samples of loss:
//   A  decreasing, 0.5 exp(-t / 4000) + 0.01     -> 0 (progress)
//   B  constant 0.5                              -> 1 (no progress, high)
//   C  constant 0.001                            -> 0 (below eps_l: done)
// with W = 2 000, rel = 0.05, eps_l = 0.1; the flag settles within a few
// thousand samples of each segment start (it reads 1 at the very start of
// C while the smoothed loss is still decaying from B, then 0).
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [no_progress, l]

op = library("optimizers.lib");
ba = library("basics.lib");

seg = ba.time / 20000;
t_seg = float(ba.time % 20000);
l = ba.selectn(3, seg, 0.5 * exp(0.0 - t_seg / 4000.0) + 0.01, 0.5, 0.001);

process = op.no_progress(2000, 0.05, 0.1, l), l;
