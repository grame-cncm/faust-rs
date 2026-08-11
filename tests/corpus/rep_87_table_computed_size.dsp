// The table extent is deliberately an arithmetic expression. C++ simplifies
// it to 64 before table extraction; faust-rs must do the same in scalar/vector
// and runtime/const table-initialization modes.
process = rdtable((4 + 4) * (10 - 2), 0.25, int(_) & 63);
