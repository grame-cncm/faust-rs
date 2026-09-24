// Environments passed to functions and accessed with ".", as the C++
// compiler allows. Regression for a faust-rs divergence where an
// environment bound to a parameter lost its definitions ("undefined symbol
// freq" on cfg.freq): forcing an environment closure to a box returned the
// bare environment node instead of a closure handle.
import("stdfaust.lib");

// An environment passed as an argument (the original repro).
play(cfg) = os.osc(cfg.freq) * cfg.gain;
low = environment { freq = 220; gain = 0.5; };

// One returned from a function and accessed, directly or through a call.
mk(f) = environment { freq = f; };
freq_of(cfg) = cfg.freq;

// A nested access through a parameter.
outer = environment { inner = environment { x = 7; }; };
get_x(cfg) = cfg.inner.x;

// A lambda parameter, a substitution on a parameter, a member function.
lambda_freq = \(cfg).(cfg.freq);
retuned(cfg) = cfg[freq = 440;].freq;
twice(cfg) = cfg.f(cfg.f(1));
triple = environment { f(x) = x * 3; };

// Two environments at once.
a = environment { v = 5; };
b = environment { v = 2; };
diff(c, d) = c.v - d.v;

process = play(low), mk(330).freq, freq_of(mk(330)), get_x(outer),
          lambda_freq(low), retuned(low), twice(triple), diff(a, b);
