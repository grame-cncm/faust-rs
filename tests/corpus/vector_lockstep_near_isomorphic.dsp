// The second lane differs at the outer binary operator. Detection must fail
// closed and preserve two ordinary recursive loops.
add_pole(g) = + ~ *(g);
sub_pole(g) = - ~ *(g);
process = add_pole(0.5), sub_pole(0.5);
