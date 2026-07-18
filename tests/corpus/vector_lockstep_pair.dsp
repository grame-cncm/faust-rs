// Two exact, independent recursive instances. Under -vec they must become one
// lockstep bundle while retaining two logical states and planar I/O channels.
pole(g) = + ~ *(g);
process = pole(0.5), pole(0.5);
