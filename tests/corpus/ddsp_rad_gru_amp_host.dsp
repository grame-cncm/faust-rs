// DDSP example (RAD, state of the art): a recurrent neural amplifier model
// trained by block-truncated backpropagation through time from the host.
//
// The model is a GRU cell with two hidden units and a linear readout, 27
// parameters, the architecture of real-time neural amp modelling (Wright &
// Valimaki 2020). The parameters are sliders; the program outputs, sample by
// sample, the squared error against a hidden amplifier (a tone filter into a
// tanh saturation) and the 27 gradients of that error with respect to the
// sliders. Nothing is learned inside the graph: the host sums each gradient
// lane over the block and takes an Adam step.
//
// Because the lanes leave the graph, the reverse sweep runs backwards over
// the whole compute() block through the recurrent cell -- the sigmoid gates,
// the tanh candidate, the two hidden states fed back -- with a zero terminal
// adjoint at the block end. The sum of a lane is therefore the exact
// gradient of the block loss with the initial state held fixed: truncated
// backpropagation through time with the block as the truncation length,
// which the host can verify against finite differences. Consumed inside the
// graph, the same `rad` would see one sample and a recurrent model could not
// be trained.
//
// Requires the directory of the Faust standard libraries on the import path
// (-I <faustlibraries>).
//
// Input: excitation. Outputs: [loss, d(loss)/d(p) for the 27 parameters].

import("stdfaust.lib");

sigm(v) = 1.0 / (1.0 + exp(0.0 - v));

// Update gate z, reset gate r, candidate c: input weights, recurrent weights
// (u<gate><to><from>), biases, linear readout. One slider per parameter --
// the parser wants a literal label -- with a fixed, deterministic initial
// value that the host reads back.
wz1 = hslider("wz1", 0.5, -4.0, 4.0, 0.0001);
wz2 = hslider("wz2", -0.4, -4.0, 4.0, 0.0001);
wr1 = hslider("wr1", 0.3, -4.0, 4.0, 0.0001);
wr2 = hslider("wr2", 0.6, -4.0, 4.0, 0.0001);
wh1 = hslider("wh1", 0.8, -4.0, 4.0, 0.0001);
wh2 = hslider("wh2", -0.7, -4.0, 4.0, 0.0001);
uz11 = hslider("uz11", 0.1, -4.0, 4.0, 0.0001);
uz12 = hslider("uz12", -0.2, -4.0, 4.0, 0.0001);
uz21 = hslider("uz21", 0.3, -4.0, 4.0, 0.0001);
uz22 = hslider("uz22", 0.05, -4.0, 4.0, 0.0001);
ur11 = hslider("ur11", 0.2, -4.0, 4.0, 0.0001);
ur12 = hslider("ur12", 0.1, -4.0, 4.0, 0.0001);
ur21 = hslider("ur21", -0.3, -4.0, 4.0, 0.0001);
ur22 = hslider("ur22", 0.4, -4.0, 4.0, 0.0001);
uh11 = hslider("uh11", 0.4, -4.0, 4.0, 0.0001);
uh12 = hslider("uh12", -0.5, -4.0, 4.0, 0.0001);
uh21 = hslider("uh21", 0.2, -4.0, 4.0, 0.0001);
uh22 = hslider("uh22", 0.3, -4.0, 4.0, 0.0001);
bz1 = hslider("bz1", 0, -4.0, 4.0, 0.0001);
bz2 = hslider("bz2", 0, -4.0, 4.0, 0.0001);
br1 = hslider("br1", 0, -4.0, 4.0, 0.0001);
br2 = hslider("br2", 0, -4.0, 4.0, 0.0001);
bh1 = hslider("bh1", 0, -4.0, 4.0, 0.0001);
bh2 = hslider("bh2", 0, -4.0, 4.0, 0.0001);
wo1 = hslider("wo1", 0.9, -4.0, 4.0, 0.0001);
wo2 = hslider("wo2", -0.6, -4.0, 4.0, 0.0001);
bo = hslider("bo", 0.0, -4.0, 4.0, 0.0001);

gru(x) = (step ~ (_, _))
with {
    step(h1, h2) = hn1, hn2
    with {
        z1 = sigm(wz1 * x + uz11 * h1 + uz12 * h2 + bz1);
        z2 = sigm(wz2 * x + uz21 * h1 + uz22 * h2 + bz2);
        r1 = sigm(wr1 * x + ur11 * h1 + ur12 * h2 + br1);
        r2 = sigm(wr2 * x + ur21 * h1 + ur22 * h2 + br2);
        c1 = ma.tanh(wh1 * x + uh11 * (r1 * h1) + uh12 * (r2 * h2) + bh1);
        c2 = ma.tanh(wh2 * x + uh21 * (r1 * h1) + uh22 * (r2 * h2) + bh2);
        hn1 = (1.0 - z1) * h1 + z1 * c1;
        hn2 = (1.0 - z2) * h2 + z2 * c2;
    };
};
model(x) = gru(x) : \(h1, h2).(wo1 * h1 + wo2 * h2 + bo);

// The hidden amplifier: a tone control into a saturation.
amp(x) = 0.8 * ma.tanh(3.0 * si.smooth(0.7, x));

loss = _ <: (amp, model) : - <: *;
params = (wz1, wz2, wr1, wr2, wh1, wh2,
          uz11, uz12, uz21, uz22, ur11, ur12, ur21, ur22, uh11, uh12, uh21, uh22,
          bz1, bz2, br1, br2, bh1, bh2, wo1, wo2, bo);

process = rad(loss, params);
