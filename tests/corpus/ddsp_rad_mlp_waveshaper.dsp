// DDSP example (RAD): a small neural network learns a waveshaper.
//
// A one-hidden-layer network with four tanh units (13 parameters) is
// trained inside the graph to imitate a soft clipper, the smallest form of
// "neural amp modelling". The loss is the squared error on the waveform,
// the optimiser Adam, and `descend_N_rad` gives the 13 gradients from one
// reverse sweep per sample: reverse mode is the mode of neural networks,
// one scalar loss and many parameters.
//
// A bus loop starts every parameter from the same value, which for a
// network would leave every hidden unit identical for ever: the model adds
// fixed, distinct offsets to the learned weights and biases, so the
// parameters are learned from zero around a deterministic initialisation.
//
// Convergence: the residual falls by more than 20 dB.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [residual, target]

import("stdfaust.lib");
op = library("optimizers.lib");

H = 4;                                    // hidden units
x = no.noise;                             // the excitation
target = 0.8 * ma.tanh(3.0 * x) + 0.1 * x; // the soft clipper to imitate

// the fixed offsets that make the units distinct at the start: unit j has
// input weight w1_0(j) and bias b1_0(j) before anything is learned
w1_0(j) = 1.0 + 0.5 * j;
b1_0(j) = -0.6 + 0.4 * j;
// hidden unit j, with its learned weight w and bias b added to the offsets
unit(j, w, b) = ma.tanh((w + w1_0(j)) * x + b + b1_0(j));

// The network as a block of its 13 parameters: (w1 x4, b1 x4, w2 x4, b2).
// hidden: inputs (w1_1 .. w1_H, b1_1 .. b1_H), paired by interleave into
// (w_j, b_j), each pair to its unit; outputs the H activations
hidden = (si.bus(H), si.bus(H)) : ro.interleave(H, 2) : par(j, H, (_, _ : unit(j)));
// net: the activations paired with the output weights w2_1 .. w2_H,
// multiplied and summed, plus the output bias b2
net = (hidden, si.bus(H), _) : ((ro.interleave(H, 2) : par(j, H, *) :> _), _) : +;
// the loss as a block of the 13 parameters: the squared error of the net's
// output against the target (the loss's input is named, `sq_err(y)`, so that
// a free `_` is not duplicated across the block's arity)
net_loss = net : sq_err with { sq_err(y) = op.mse(y, target); };

// descend_N_rad(N, loss, engine, lo, hi, init, reset): the 13 parameters as
// a bus, the 13 gradients from one reverse sweep per sample, one Adam per
// parameter (rate 0.003, the moments of the paper, eps 1e-8); parameters in
// [-4, 4], starting at 0 (the offsets carry the initialisation); no reset
p = op.descend_N_rad(3 * H + 1, net_loss, op.adam_g(0.003, 0.9, 0.999, 1e-8), -4.0, 4.0, 0.0, 0.0);

// ---- outputs
// [0] the residual, target - net on the same noise: from 0.105 rms at the
//     start (the offsets alone) to under 0.004 rms, more than 20 dB down
// [1] the target, for reference
process = target - (p : net), target;
