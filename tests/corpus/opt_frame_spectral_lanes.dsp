// optimizers.lib: identities of `frame_spectral_loss` on an 8-sample frame
// of constants: the loss of a frame with itself is 0; the loss is
// symmetric bit for bit; and since the magnitudes scale with the frame, the
// loss of twice the frame against the frame is close to the loss of silence
// against the frame, (2|T| - |T|)^2 = (0 - |T|)^2, up to the floor `eps`
// under the square root, which gives silence a magnitude of sqrt(eps)
// instead of 0: the gap is 2 sqrt(eps) sum |T|, about 3e-5 of the loss.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [loss(t, t), loss(2t, t) - loss(t, 2t), loss(2t, t) - loss(0, t), loss(0, t)]

op = library("optimizers.lib");

frame = 0.3, -0.5, 0.8, 0.1, -0.2, 0.6, -0.9, 0.4;
twice = frame : par(i, 8, *(2.0));
silence = frame : par(i, 8, *(0.0));
loss = op.frame_spectral_loss(8, 0.000000001);
l_tt = (frame, frame) : loss;
l_2t = (twice, frame) : loss;
l_t2 = (frame, twice) : loss;
l_0t = (silence, frame) : loss;

process = l_tt, l_2t - l_t2, l_2t - l_0t, l_0t;
