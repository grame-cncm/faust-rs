// The differentiable IIR filter of "Faust Autodiff: Towards Audio
// Domain-Specific Machine Learning" (T. Rushton, AES AIMLA 2025,
// listing 5), order N, written with the paper's own routing:
//   v[n] = x[n] - sum_{l<N}  a(l+1) * v[n-1-l]      (the recursion)
//   y[n] =        sum_{l<=N} b(l)   * v[n-l]        (the taps)
// differentiated with respect to its 2N+1 coefficients.
//
// The paper cannot use fi.iir: its `ma.sub` is an unapplied abstraction the
// source-level transformation sees as a constant, so the subtraction is
// re-routed with `_` and `!` (sub below), and the coefficients enter through
// a wrapper (diffInN) as inputs. Here the coefficients are sliders taken as
// seeds and rad differentiates the signal graph after propagation. The body
// is recursive, so the reverse sweep is a BlockReverseAD over the compute()
// block: each gradient lane holds per-sample contributions whose sum over the
// block is the gradient of the block sum of y, as the runtime test checks.
//
// Outputs: [y, dy/da1, dy/da2, dy/db0, dy/db1, dy/db2] (block contributions)
ro = library("routes.lib");

N = 2;

dot(n) = ro.interleave(n, 2) : par(i, n, *) :> _;
sub = _, _ <: !, _, _, ! : -;
fir(n) = (_ <: par(l, n, seq(m, l, mem))), par(l, n, _) : dot(n);
iir = ((sub, par(l, N, _)) ~ fir(N)), par(l, N + 1, _)
    : _, par(l, N, !), par(l, N + 1, _)
    : fir(N + 1);

a(i) = hslider("a%i", -0.5 + 0.75 * (i - 1), -2, 2, 0.001);
b(i) = hslider("b%i", 0.3 - 0.1 * i, -2, 2, 0.001);
coeffs = par(i, N, a(i + 1)), par(i, N + 1, b(i));

process = rad((_, coeffs : iir), coeffs);
