// controls.lib: every public function instantiated through the `#### Test`
// entry of its documentation, so the documented examples are compiled by the
// test suite. Generated from the library documentation; regenerate when a
// Test entry changes.
//
// Requires -I libraries (project-local controls.lib, which imports nothing).
//
// Inputs: one (nonfinite_test). Outputs: the outputs of every Test entry, in
// library order.

ct = library("controls.lib");

count_test = ct.count(hslider("count:a", 0.5, 0, 1, 0.1) + button("count:b"));
widget_test = ct.widget(0, hslider("widget:a", 0.5, 0, 1, 0.1) * 2);
init_test = ct.init(0, hslider("init:a", 0.5, 0, 1, 0.1));
lo_test = ct.lo(0, hslider("lo:a", 0.5, 0, 1, 0.1));
hi_test = ct.hi(0, hslider("hi:a", 0.5, 0, 1, 0.1));
step_test = ct.step(0, hslider("step:a", 0.5, 0, 1, 0.1));
inits_test = ct.inits(hslider("inits:a", 0.5, 0, 1, 0.1) + hslider("inits:b", 0.25, 0, 1, 0.1));
map_test = ct.map(f, e)
with { e = hslider("map:a", 0.5, 0, 1, 0.1) * 10 + hslider("map:b", 0.25, 0, 1, 0.1); f(i, w) = w * (i + 1); };
external_test = (0.5, 0.25) : ct.external(hslider("external:a", 0, 0, 1, 0.1) * 10 + hslider("external:b", 0, 0, 1, 0.1));
normalized_test = (0.5, 1) : ct.normalized(hslider("normalized:f", 440, 20, 20000, 1) + hslider("normalized:g", 0, -1, 1, 0.01));
cv_test = (0.1, -0.1) : ct.cv(1, hslider("cv:a", 0.5, 0, 1, 0.1) * 10 + hslider("cv:b", 0.25, 0, 1, 0.1));
smooth_test = ct.smooth(0.01, hslider("smooth:a", 0.5, 0, 1, 0.1) * 10);
smoother_test = 1 : ct.smoother(0.01);
relabel_test = ct.relabel(wdg, hslider("relabel:a", 0.5, 0, 1, 0.1) * 10)
with { wdg(i, v, a, b, s) = vslider("relabel:P%i", v, a, b, s); };
knobs_test = hgroup("knobs", ct.knobs(hslider("knobs:a", 0.5, 0, 1, 0.1) * 10));
morph_test = ct.morph(0.5, (1, 0), hslider("morph:a", 0.5, 0, 1, 0.1) * 10 + hslider("morph:b", 0.25, 0, 1, 0.1));
randomize_test = ct.randomize(button("randomize:new"), 1, hslider("randomize:a", 0.5, 0, 1, 0.1) * 10);
offset_test = ct.offset(0.1, hslider("offset:a", 0.5, 0, 1, 0.1) * 10);
sweep_test = ct.sweep(1000, hslider("sweep:a", 0.5, 0, 1, 0.1) + hslider("sweep:b", 0.5, 0, 1, 0.1));
sweep_one_test = ct.sweep_one(1, 1000, hslider("sweep_one:a", 0.5, 0, 1, 0.1) + hslider("sweep_one:b", 0.5, 0, 1, 0.1));
nonfinite_test = (1, 1.0 / (_ - 1)) : ct.nonfinite(2);
gradient_fad_test = 1 : ct.gradient_fad(*(hslider("gradient_fad:a", 0.5, 0, 1, 0.1)));
gradient_rad_test = 1 : ct.gradient_rad(*(hslider("gradient_rad:a", 0.5, 0, 1, 0.1)));

process = count_test, widget_test, init_test, lo_test, hi_test, step_test, inits_test, map_test, external_test, normalized_test, cv_test, smooth_test, smoother_test, relabel_test, knobs_test, morph_test, randomize_test, offset_test, sweep_test, sweep_one_test, nonfinite_test, gradient_fad_test, gradient_rad_test;
