// The artificial neuron of "Faust Autodiff: Towards Audio Domain-Specific
// Machine Learning" (T. Rushton, AES AIMLA 2025, listing 4):
//   y = sigmoid(w . x + b)
// on NW input signals, differentiated with respect to its NW weights and
// its bias.
//
// The paper differentiates at the source level, by pattern matching on the
// box algebra: the weights and the bias have to leave the neuron and enter
// through a wrapper (diffInN) as extra inputs, and the dot product needs a
// hand-written interleave because `route` cannot be pattern-matched. Here
// the weights and the bias are sliders taken as seeds, the dot product goes
// through ro.interleave (a `route`), and fad differentiates the signal graph
// after propagation, where neither obstacle exists.
//
// Outputs: [y, dy/dw0, dy/dw1, dy/dw2, dy/db]
//   dy/dwi = y * (1 - y) * xi
//   dy/db  = y * (1 - y)
ro = library("routes.lib");

NW = 3;

sigmoid = *(-1) : 1 / (1 + exp);
dot(n) = ro.interleave(n, 2) : par(i, n, *) :> _;
neuron(n, activation) = WX + b : activation
with {
    WX = dot(n);
    b = _;
};

w(i) = hslider("w%i", 0.5 - 0.75 * i + 0.5 * i * i, -2, 2, 0.001);
bias = hslider("b", 0.1, -2, 2, 0.001);
weights = par(i, NW, w(i)), bias;

process = fad((par(i, NW, _), weights : neuron(NW, sigmoid)), weights);
