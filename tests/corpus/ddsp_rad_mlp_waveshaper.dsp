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

H = 4;
x = no.noise;
target = 0.8 * ma.tanh(3.0 * x) + 0.1 * x;

w1_0(j) = 1.0 + 0.5 * j;
b1_0(j) = -0.6 + 0.4 * j;
unit(j, w, b) = ma.tanh((w + w1_0(j)) * x + b + b1_0(j));

// The network as a block of its 13 parameters: (w1 x4, b1 x4, w2 x4, b2).
hidden = (si.bus(H), si.bus(H)) : ro.interleave(H, 2) : par(j, H, (_, _ : unit(j)));
net = (hidden, si.bus(H), _) : ((ro.interleave(H, 2) : par(j, H, *) :> _), _) : +;
net_loss = net : sq_err with { sq_err(y) = op.mse(y, target); };

p = op.descend_N_rad(3 * H + 1, net_loss, op.adam_g(0.003, 0.9, 0.999, 1e-8), -4.0, 4.0, 0.0, 0.0);

process = target - (p : net), target;
