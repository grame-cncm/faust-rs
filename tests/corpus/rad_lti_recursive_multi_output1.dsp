// Accepted RAD E1 fixture: two independent strict-LTI recursions.
//
// This pins the public multi-output layout for accepted recursive RAD:
// [y0, y1, dp, dq].
//p = 0.5;

//q = 0.25;

p = hslider("p", 0.5, 0.0, 1.0, 0.01);
q = hslider("q", 0.25, 0.0, 1.0, 0.01);
process = rad(((2 : + ~ *(p)), (3 : + ~ *(q))), (p, q));


//p = hslider("p", 0.5, 0.0, 1.0, 0.01);
//process = rad((2 : + ~ *(p)), p);