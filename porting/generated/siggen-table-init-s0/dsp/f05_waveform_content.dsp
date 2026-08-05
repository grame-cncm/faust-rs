// F05 — waveform as generator content. Upstream gives it a sub-container owning
// f<Sub>Wave0 plus a copy loop; faust-rs folds it today (const mode keeps that).
wv = waveform{10.0, 20.0, 30.0, 40.0, 50.0};
process = rdtable(5, wv : (!, _), 0);
