// Pattern-matching rules are tried in textual order and the first rule that
// matches wins, as in the C++ compiler: a general rule written before a
// specific one hides it. Regression for a faust-rs divergence where the
// specific rule won (the automaton's rule lists were appended instead of
// merged in rule order).

// Definitions by cases, general rule first: f(0) is 1, not 2.
f(n) = 1;
f(0) = 2;

// The same with a case expression.
first_wins = case { (n) => 1; (0) => 2; };

// Specific rule first: g(0) takes it, g(5) falls to the general one.
g(0) = 10;
g(n) = 20;

// Two arguments: h(0, 0) matches all three rules, the first one wins.
h(x, 0) = 100;
h(0, y) = 200;
h(x, y) = 300;

// A variable before a structural pattern: p((1, 2)) is 1, not 2.
p(x) = 1;
p((a, b)) = 2;

process = f(0), f(3), first_wins(0), first_wins(7), g(0), g(5), h(0, 0), h(1, 0), h(0, 1), h(1, 1), p((1, 2)), p(3);
