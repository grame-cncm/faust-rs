// optimizers.lib: every public function instantiated through the `#### Test`
// entry of its documentation, so the documented examples are compiled by the
// test suite. Generated from the library documentation; regenerate when a
// Test entry changes.
//
// Requires -I libraries (project-local optimizers.lib; no stdfaust.lib).
//
// Outputs: the outputs of every Test entry, in library order.

op = library("optimizers.lib");

clip_test = hslider("clip:x", 0, -2, 2, 0.01) : op.clip(-1.0, 1.0);
sgn_test = hslider("sgn:x", 0, -1, 1, 0.01) : op.sgn;
ema_test = hslider("ema:x", 0, -1, 1, 0.01) : op.ema(0.99);
ema_bc_test = hslider("ema_bc:x", 0, -1, 1, 0.01) : op.ema_bc(0.99);
pstate_test = (op.pstate(1.0, button("pstate:reset")) : *(0.999)) ~ _;
polyak_test = hslider("polyak:p", 0, -1, 1, 0.01) : op.polyak(0.999);
mse_test = op.mse(hslider("mse:y", 0, -1, 1, 0.01), 0.5);
pseudo_huber_test = op.pseudo_huber(0.05, hslider("pseudo_huber:y", 0, -1, 1, 0.01), 0.5);
logcosh_test = op.logcosh(hslider("logcosh:y", 0, -1, 1, 0.01), 0.5);
energy_loss_test = op.energy_loss(0.999, hslider("energy_loss:y", 0, -1, 1, 0.01), 0.5);
log_energy_loss_test = op.log_energy_loss(0.999, 1e-9, hslider("log_energy_loss:y", 0, -1, 1, 0.01), 0.5);
l2_test = op.l2(0.01, hslider("l2:p", 0, -1, 1, 0.01));
l1s_test = op.l1s(0.01, 0.001, hslider("l1s:p", 0, -1, 1, 0.01));
poles_from_reflection_test = op.poles_from_reflection(hslider("poles:k1", 0, -0.99, 0.99, 0.01), 0.5);
reflection_from_poles_test = op.reflection_from_poles(-1.0, 0.4);
sigmoid_map_test = op.sigmoid_map(20.0, 20000.0, hslider("sigmoid_map:u", 0, -6, 6, 0.01));
clip_g_test = hslider("clip_g:g", 0, -2, 2, 0.01) : op.clip_g(1.0);
softclip_g_test = hslider("softclip_g:g", 0, -2, 2, 0.01) : op.softclip_g(1.0);
gate_g_test = hslider("gate_g:g", 0, -1, 1, 0.01) : op.gate_g(checkbox("gate_g:learn"));
lr_exp_test = op.lr_exp(0.01, 0.0001, 48000.0);
lr_cos_test = op.lr_cos(0.01, 0.0001, 48000.0);
warmup_test = 0.01 * op.warmup(4800.0);
lms_test = op.lms(0.01, hslider("lms:r", 0, -1, 1, 0.01), 0.5);
nlms_test = op.nlms(0.02, 1e-6, 0.99, hslider("nlms:r", 0, -1, 1, 0.01), 0.5);
gn1_test = op.gn1(0.01, 1e-6, 0.99, hslider("gn1:r", 0, -1, 1, 0.01), 0.5);
sgd_test = op.sgd(0.01, 0.1, hslider("sgd:r", 0, -1, 1, 0.01), 0.5);
adam_test = op.adam(0.01, hslider("adam:r", 0, -1, 1, 0.01), 0.5);
rmsprop_test = op.rmsprop(0.002, hslider("rmsprop:r", 0, -1, 1, 0.01), 0.5);
nadam_test = op.nadam(0.01, hslider("nadam:r", 0, -1, 1, 0.01), 0.5);
sign_sgd_test = op.sign_sgd(0.001, hslider("sign_sgd:r", 0, -1, 1, 0.01), 0.5);
sgd_g_test = op.sgd_g(0.01, hslider("sgd_g:g", 0, -1, 1, 0.01));
momentum_g_test = op.momentum_g(0.01, 0.9, hslider("momentum_g:g", 0, -1, 1, 0.01));
nesterov_g_test = op.nesterov_g(0.01, 0.9, hslider("nesterov_g:g", 0, -1, 1, 0.01));
adam_g_test = op.adam_g(0.01, 0.9, 0.999, 1e-8, hslider("adam_g:g", 0, -1, 1, 0.01));
nadam_g_test = op.nadam_g(0.01, 0.9, 0.999, 1e-8, hslider("nadam_g:g", 0, -1, 1, 0.01));
amsgrad_g_test = op.amsgrad_g(0.01, 0.9, 0.999, 1e-8, hslider("amsgrad_g:g", 0, -1, 1, 0.01));
adabelief_g_test = op.adabelief_g(0.01, 0.9, 0.999, 1e-8, hslider("adabelief_g:g", 0, -1, 1, 0.01));
rmsprop_g_test = op.rmsprop_g(0.01, 0.999, 1e-8, hslider("rmsprop_g:g", 0, -1, 1, 0.01));
adagrad_g_test = op.adagrad_g(0.01, 1e-8, hslider("adagrad_g:g", 0, -1, 1, 0.01));
lion_g_test = op.lion_g(0.001, 0.9, 0.99, hslider("lion_g:g", 0, -1, 1, 0.01));
sign_g_test = op.sign_g(0.001, hslider("sign_g:g", 0, -1, 1, 0.01));
lsq_1D_test = op.lsq_1D(\(p, x).(p * x), op.nlms(0.02, 1e-6, 0.99), -4, 4, 0, 0, 0.5 * x, x)
with { x = hslider("lsq_1D:x", 0, -1, 1, 0.01); };
lsq_2D_test = op.lsq_2D(\(a, b, x).(a * x + b), op.lms(0.01), op.lms(0.01), -4, 4, -4, 4, 0, 0, 0, 0.5 * x + 0.1, x)
with { x = hslider("lsq_2D:x", 0, -1, 1, 0.01); };
lsq_3D_test = op.lsq_3D(\(h0, h1, h2, x).(h0 * x + h1 * x' + h2 * x''), op.nlms(0.02, 1e-6, 0.99), op.nlms(0.02, 1e-6, 0.99), op.nlms(0.02, 1e-6, 0.99), -4, 4, -4, 4, -4, 4, 0, 0, 0, 0, 0.5 * x + 0.3 * x' - 0.2 * x'', x)
with { x = hslider("lsq_3D:x", 0, -1, 1, 0.01); };
lsq_4D_test = op.lsq_4D(\(a, b, c, d, x).(a * x + b * x' + c * x'' + d), op.lms(0.01), op.lms(0.01), op.lms(0.01), op.lms(0.01), -4, 4, -4, 4, -4, 4, -4, 4, 0, 0, 0, 0, 0, 0.5 * x + 0.3 * x' - 0.2 * x'' + 0.1, x)
with { x = hslider("lsq_4D:x", 0, -1, 1, 0.01); };
lsq_5D_test = op.lsq_5D(\(a, b, c, d, e, x).(a * x + b * x' + c * x'' + d * x''' + e), op.lms(0.01), op.lms(0.01), op.lms(0.01), op.lms(0.01), op.lms(0.01), -4, 4, -4, 4, -4, 4, -4, 4, -4, 4, 0, 0, 0, 0, 0, 0, 0.5 * x + 0.3 * x' - 0.2 * x'' + 0.1 * x''' + 0.1, x)
with { x = hslider("lsq_5D:x", 0, -1, 1, 0.01); };
optimize_1D_test = op.optimize_1D(\(p, x).(p * x), op.adam(0.01), -4, 4, 0.5 * x, x)
with { x = hslider("optimize_1D:x", 0, -1, 1, 0.01); };
optimize_2D_test = op.optimize_2D(\(a, b, x).(a * x + b), op.adam(0.01), op.adam(0.01), -4, 4, -4, 4, 0.5 * x + 0.1, x)
with { x = hslider("optimize_2D:x", 0, -1, 1, 0.01); };
optimize_3D_test = op.optimize_3D(\(h0, h1, h2, x).(h0 * x + h1 * x' + h2 * x''), op.adam(0.01), op.adam(0.01), op.adam(0.01), -4, 4, -4, 4, -4, 4, 0.5 * x + 0.3 * x' - 0.2 * x'', x)
with { x = hslider("optimize_3D:x", 0, -1, 1, 0.01); };
optimize_4D_test = op.optimize_4D(\(a, b, c, d, x).(a * x + b * x' + c * x'' + d), op.adam(0.01), op.adam(0.01), op.adam(0.01), op.adam(0.01), -4, 4, -4, 4, -4, 4, -4, 4, 0.5 * x + 0.3 * x' - 0.2 * x'' + 0.1, x)
with { x = hslider("optimize_4D:x", 0, -1, 1, 0.01); };
optimize_5D_test = op.optimize_5D(\(a, b, c, d, e, x).(a * x + b * x' + c * x'' + d * x''' + e), op.adam(0.01), op.adam(0.01), op.adam(0.01), op.adam(0.01), op.adam(0.01), -4, 4, -4, 4, -4, 4, -4, 4, -4, 4, 0.5 * x + 0.3 * x' - 0.2 * x'' + 0.1 * x''' + 0.1, x)
with { x = hslider("optimize_5D:x", 0, -1, 1, 0.01); };
descend_1D_test = op.descend_1D(\(p).(op.mse(p * x, 0.5 * x)), op.adam_g(0.01, 0.9, 0.999, 1e-8), -4, 4, 0, 0)
with { x = hslider("descend_1D:x", 0, -1, 1, 0.01); };
descend_2D_test = op.descend_2D(\(a, b).(op.mse(a * x + b, 0.5 * x + 0.1)), op.adam_g(0.01, 0.9, 0.999, 1e-8), op.adam_g(0.01, 0.9, 0.999, 1e-8), -4, 4, -4, 4, 0, 0, 0)
with { x = hslider("descend_2D:x", 0, -1, 1, 0.01); };
descend_3D_test = op.descend_3D(\(h0, h1, h2).(op.mse(h0 * x + h1 * x' + h2 * x'', 0.5 * x + 0.3 * x' - 0.2 * x'')), op.adam_g(0.01, 0.9, 0.999, 1e-8), op.adam_g(0.01, 0.9, 0.999, 1e-8), op.adam_g(0.01, 0.9, 0.999, 1e-8), -4, 4, -4, 4, -4, 4, 0, 0, 0, 0)
with { x = hslider("descend_3D:x", 0, -1, 1, 0.01); };
descend_4D_test = op.descend_4D(\(a, b, c, d).(op.mse(a * x + b * x' + c * x'' + d, 0.5 * x + 0.3 * x' - 0.2 * x'' + 0.1)), op.adam_g(0.01, 0.9, 0.999, 1e-8), op.adam_g(0.01, 0.9, 0.999, 1e-8), op.adam_g(0.01, 0.9, 0.999, 1e-8), op.adam_g(0.01, 0.9, 0.999, 1e-8), -4, 4, -4, 4, -4, 4, -4, 4, 0, 0, 0, 0, 0)
with { x = hslider("descend_4D:x", 0, -1, 1, 0.01); };
descend_5D_test = op.descend_5D(\(a, b, c, d, e).(op.mse(a * x + b * x' + c * x'' + d * x''' + e, 0.5 * x + 0.3 * x' - 0.2 * x'' + 0.1 * x''' + 0.1)), op.adam_g(0.01, 0.9, 0.999, 1e-8), op.adam_g(0.01, 0.9, 0.999, 1e-8), op.adam_g(0.01, 0.9, 0.999, 1e-8), op.adam_g(0.01, 0.9, 0.999, 1e-8), op.adam_g(0.01, 0.9, 0.999, 1e-8), -4, 4, -4, 4, -4, 4, -4, 4, -4, 4, 0, 0, 0, 0, 0, 0)
with { x = hslider("descend_5D:x", 0, -1, 1, 0.01); };
lm_2D_test = op.lm_2D(\(a, b, x).(a * x + b), 0.01, 0.1, 0.99, -4, 4, -4, 4, 0, 0, 0, 0.5 * x + 0.1, x)
with { x = hslider("lm_2D:x", 0, -1, 1, 0.01); };
lm_3D_test = op.lm_3D(\(h0, h1, h2, x).(h0 * x + h1 * x' + h2 * x''), 0.01, 0.1, 0.99, -4, 4, -4, 4, -4, 4, 0, 0, 0, 0, 0.5 * x + 0.3 * x' - 0.2 * x'', x)
with { x = hslider("lm_3D:x", 0, -1, 1, 0.01); };
frame_sum_test = hslider("frame_sum:x", 0, -1, 1, 0.01) : op.frame_sum(clock)
with { clock = ((+(1) : %(64)) ~ _) == 0; };
frame_count_test = op.frame_count(clock)
with { clock = ((+(1) : %(64)) ~ _) == 0; };
frame_mean_test = hslider("frame_mean:x", 0, -1, 1, 0.01) : op.frame_mean(clock)
with { clock = ((+(1) : %(64)) ~ _) == 0; };
descend_1D_clocked_test = op.descend_1D_clocked(clock, \(p).(op.mse(p * x, 0.5 * x)), op.sgd_g(0.5), -4, 4, 0, 0)
with { x = hslider("descend_1D_clocked:x", 0, -1, 1, 0.01); clock = ((+(1) : %(64)) ~ _) == 0; };
descend_2D_clocked_test = op.descend_2D_clocked(clock, \(a, b).(op.mse(a * x + b, 0.5 * x + 0.1)), op.sgd_g(0.5), op.sgd_g(0.5), -4, 4, -4, 4, 0, 0, 0)
with { x = hslider("descend_2D_clocked:x", 0, -1, 1, 0.01); clock = ((+(1) : %(64)) ~ _) == 0; };
descend_3D_clocked_test = op.descend_3D_clocked(clock, \(h0, h1, h2).(op.mse(h0 * x + h1 * x' + h2 * x'', 0.5 * x + 0.3 * x' - 0.2 * x'')), op.sgd_g(0.5), op.sgd_g(0.5), op.sgd_g(0.5), -4, 4, -4, 4, -4, 4, 0, 0, 0, 0)
with { x = hslider("descend_3D_clocked:x", 0, -1, 1, 0.01); clock = ((+(1) : %(64)) ~ _) == 0; };
descend_4D_clocked_test = op.descend_4D_clocked(clock, \(a, b, c, d).(op.mse(a * x + b * x' + c * x'' + d, 0.5 * x + 0.3 * x' - 0.2 * x'' + 0.1)), op.sgd_g(0.5), op.sgd_g(0.5), op.sgd_g(0.5), op.sgd_g(0.5), -4, 4, -4, 4, -4, 4, -4, 4, 0, 0, 0, 0, 0)
with { x = hslider("descend_4D_clocked:x", 0, -1, 1, 0.01); clock = ((+(1) : %(64)) ~ _) == 0; };
descend_5D_clocked_test = op.descend_5D_clocked(clock, \(a, b, c, d, e).(op.mse(a * x + b * x' + c * x'' + d * x''' + e, 0.5 * x + 0.3 * x' - 0.2 * x'' + 0.1 * x''' + 0.1)), op.sgd_g(0.5), op.sgd_g(0.5), op.sgd_g(0.5), op.sgd_g(0.5), op.sgd_g(0.5), -4, 4, -4, 4, -4, 4, -4, 4, -4, 4, 0, 0, 0, 0, 0, 0)
with { x = hslider("descend_5D_clocked:x", 0, -1, 1, 0.01); clock = ((+(1) : %(64)) ~ _) == 0; };
newton_step_test = op.newton_step(\(y).(y * y - 2.0), hslider("newton_step:y", 1, 0.5, 2, 0.01));
newton_test = op.newton(6, \(y).(y * y * y + y - x), 0.0)
with { x = hslider("newton:x", 0, -1, 1, 0.01); };

process = clip_test, sgn_test, ema_test, ema_bc_test, pstate_test, polyak_test, mse_test, pseudo_huber_test, logcosh_test, energy_loss_test, log_energy_loss_test, l2_test, l1s_test, poles_from_reflection_test, reflection_from_poles_test, sigmoid_map_test, clip_g_test, softclip_g_test, gate_g_test, lr_exp_test, lr_cos_test, warmup_test, lms_test, nlms_test, gn1_test, sgd_test, adam_test, rmsprop_test, nadam_test, sign_sgd_test, sgd_g_test, momentum_g_test, nesterov_g_test, adam_g_test, nadam_g_test, amsgrad_g_test, adabelief_g_test, rmsprop_g_test, adagrad_g_test, lion_g_test, sign_g_test, lsq_1D_test, lsq_2D_test, lsq_3D_test, lsq_4D_test, lsq_5D_test, optimize_1D_test, optimize_2D_test, optimize_3D_test, optimize_4D_test, optimize_5D_test, descend_1D_test, descend_2D_test, descend_3D_test, descend_4D_test, descend_5D_test, lm_2D_test, lm_3D_test, frame_sum_test, frame_count_test, frame_mean_test, descend_1D_clocked_test, descend_2D_clocked_test, descend_3D_clocked_test, descend_4D_clocked_test, descend_5D_clocked_test, newton_step_test, newton_test;
