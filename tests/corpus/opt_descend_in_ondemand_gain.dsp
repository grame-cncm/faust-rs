// optimizers.lib: the whole optimizer inside an `ondemand` block -- the
// tutorial's "the whole optimizer, clocked" (section 11.1): `descend_1D`
// placed in a block fired every 64 samples, learning a gain on the block's
// inputs. Everything, gradient included, runs in fire time.
//
// Regression guard: `pstate`'s first-sample gate must not read `ba.time`
// (its `mem`, captured across the clock boundary, read 0 at every firing
// and re-initialised the parameter each time: the gain stayed at 0.017).
//
// Convergence: gain 0 -> 0.7 in a few hundred firings; residual -> 0.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [residual_L, residual_R]

import("stdfaust.lib");
op = library("optimizers.lib");
il = library("interleave.lib");

x = no.noise;
target = 0.7 * x;
learn(xi, ti) = op.descend_1D(\(g).(op.mse(g * xi, ti)), op.adam_g(0.02, 0.9, 0.999, 1e-8), -4.0, 4.0, 0.0, 0.0);
g = (il.frame_clock(64), x, target) : ondemand(learn);

process = (target - g * x) <: _, _;
