// Mixed AD over a delay: fad(rad(x', x), x). The inner rad is a block sweep
// (SigBlockReverseAD), and fad differentiates its carrier: the tangent of a
// primal is the tangent of the body, the tangent of a gradient the gradient
// of a second carrier over that tangent.
// Outputs = 4: [x', its tangent, the gradient per sample, its tangent].
x = hslider("x", 0.0, -1.0, 1.0, 0.01);
process = fad(rad(x', x), x);
