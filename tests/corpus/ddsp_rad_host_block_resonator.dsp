// DDSP example (RAD): block gradients of a resonator, handed to a host.
//
// The two denominator coefficients of a resonant filter are sliders; the
// program outputs, sample by sample, the squared error against a hidden
// resonator and the two gradients of that error with respect to the
// sliders. Nothing is learned inside the graph: a host (the Rust test
// `ddsp_examples.rs`, a plugin, a Python script) sums the gradient lanes
// over each block and updates the sliders with Adam.
//
// Because the gradient lanes leave the graph, the reverse sweep runs
// backwards over the whole compute() block: the adjoint of the resonator's
// state is carried from sample to sample within the block, so the sum of a
// lane is the exact gradient of the block's loss (zero terminal adjoint at
// the block end), which the host can check against finite differences.
// Consumed inside the graph, the same `rad` would see one sample.
//
// Requires the directory of the Faust standard libraries on the import path
// (-I <faustlibraries>).
//
// Input: excitation. Outputs: [loss, dloss/da1, dloss/da2] per sample.

import("stdfaust.lib");

a1 = hslider("a1", -0.8, -1.99, 1.99, 0.0001);
a2 = hslider("a2", 0.5, -0.999, 0.999, 0.0001);
a1_star = -1.2;
a2_star = 0.72;

resonator(c1, c2, sig) = sig : fi.tf2(1.0, 0.0, 0.0, c1, c2);
loss = _ <: (resonator(a1_star, a2_star), resonator(a1, a2)) : - <: *;

process = rad(loss, (a1, a2));
