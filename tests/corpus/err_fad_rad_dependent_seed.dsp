// fad over a rad lowered to a block sweep, whose seed is computed from the fad
// seed: the second sweep would walk the tangent of the seed, here the seed
// itself. Refused in propagation (FRS-PROP-0005).
z = hslider("z", 0.3, -1.0, 1.0, 0.01);
s = exp(z);
process = fad(rad(s * s', s), z);
