// Fixture for the lexer differential (phase L0 of
// porting/lexer-combined-dfa-port-plan-2026-08-06-en.md).
//
// This file is *meant* not to lex. `faustlexer.l` ends with a catch-all
// `. 'EXTRA'`, so no input can fail in the INITIAL condition — unknown
// characters become EXTRA tokens the parser rejects later. The exclusive
// `lst` condition has no such catch-all, so an unrecognized attribute inside
// `<listing ...>` is the one shape that makes lexing itself stop.
//
// Without it, the differential compares only successful streams and the
// error-offset comparison is dead code.
<mdoc>
<listing zzz="x" />
</mdoc>
process = _;
