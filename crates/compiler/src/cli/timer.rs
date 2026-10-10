//! CLI timing and timeout support.
//!
//! [`CompilationTimer`] tracks total wall-clock time for one CLI invocation.
//! It complements the compiler library's internal timing sink by enforcing the
//! user-facing `--timeout` limit and printing the aggregate `[total]` line when
//! `--compilation-time` is enabled.

use std::time::{Duration, Instant};

use compiler::{Diagnostic, DiagnosticBundle, Severity, Stage};

use super::args::{ErrorFormat, ErrorVerbosity};
use super::diagnostics::format_diagnostics_json_with_verbosity;

/// The deadline of `--timeout N`: `None` for 0, which disables it.
///
/// The phase timer and the watchdog read it the same way. Until 2026-10-10
/// the watchdog took 0 as "disabled" while the phase timer rejected every
/// compilation against a zero-second limit.
pub(crate) fn deadline(timeout_secs: u64) -> Option<Duration> {
    (timeout_secs > 0).then(|| Duration::from_secs(timeout_secs))
}

/// Whether `elapsed` is past `deadline`; never for a disabled one.
pub(crate) fn expired(elapsed: Duration, deadline: Option<Duration>) -> bool {
    deadline.is_some_and(|limit| elapsed > limit)
}

/// The `FRS-COMP-0008` diagnostic of a compilation that exceeded its limit,
/// after a phase when the phase timer saw it, or from the watchdog.
pub(crate) fn timeout_diagnostic(
    limit_secs: u64,
    elapsed_secs: Option<f64>,
    phase: Option<&str>,
) -> Diagnostic {
    let message = match (elapsed_secs, phase) {
        (Some(elapsed), Some(phase)) => format!(
            "compilation timeout ({elapsed:.1}s > {limit_secs}s limit) after phase '{phase}'"
        ),
        _ => format!("compilation timeout ({limit_secs}s limit exceeded)"),
    };
    Diagnostic::new(
        Severity::Error,
        Stage::Compiler,
        diagnostics::codes::COMP_TIMEOUT,
        message,
    )
    .with_note(format!(
        "cause: the compilation ran longer than the `--timeout` limit of {limit_secs} s"
    ))
    .with_help("raise `--timeout`, or set it to 0 to disable it; a recursion whose argument never becomes a constant can also run until the limit")
}

/// Reports a timeout and exits with status 1.
///
/// Human mode prints the historical `ERROR: compilation timeout ...` line on
/// stderr. JSON mode prints one diagnostics-v2 document on stdout, as every
/// other failure does, so that a consumer reading the JSON channel gets one.
pub(crate) fn report_timeout(
    diagnostic: Diagnostic,
    format: ErrorFormat,
    verbosity: ErrorVerbosity,
) -> ! {
    print_timeout(&diagnostic, format, verbosity);
    std::process::exit(1);
}

/// The output of [`report_timeout`], without the exit.
fn print_timeout(diagnostic: &Diagnostic, format: ErrorFormat, verbosity: ErrorVerbosity) {
    match format {
        ErrorFormat::Human => eprintln!("ERROR: {}", diagnostic.message),
        ErrorFormat::Json => {
            let mut bundle = DiagnosticBundle::new();
            bundle.push(diagnostic.clone());
            println!(
                "{}",
                format_diagnostics_json_with_verbosity(&bundle, verbosity)
            );
        }
    }
}

/// Tracks elapsed time across compilation phases and enforces a global timeout.
pub struct CompilationTimer {
    /// Absolute start time of the compilation run.
    start: Instant,
    /// The `--timeout` limit in seconds, 0 when disabled.
    timeout_secs: u64,
    /// When `true`, the total timing is printed to stderr. Internal compiler
    /// phase timings are reported through `compiler::Compiler::with_timing_sink`.
    display: bool,
    /// How a timeout is reported.
    format: ErrorFormat,
    verbosity: ErrorVerbosity,
}

impl CompilationTimer {
    /// Creates a new timer. `timeout_secs` sets the limit, 0 disabling it;
    /// `display` controls whether the total timing is printed to stderr;
    /// `format` and `verbosity` how a timeout is reported.
    pub fn new(
        timeout_secs: u64,
        display: bool,
        format: ErrorFormat,
        verbosity: ErrorVerbosity,
    ) -> Self {
        Self {
            start: Instant::now(),
            timeout_secs,
            display,
            format,
            verbosity,
        }
    }

    /// Mark the end of a compilation phase and abort if the global timeout has
    /// been exceeded.
    pub fn phase(&mut self, name: &str) {
        let elapsed = self.start.elapsed();
        if expired(elapsed, deadline(self.timeout_secs)) {
            report_timeout(
                timeout_diagnostic(self.timeout_secs, Some(elapsed.as_secs_f64()), Some(name)),
                self.format,
                self.verbosity,
            );
        }
    }

    /// Print the total compilation time (only when `--compilation-time` is active).
    pub fn total(&self) {
        if self.display {
            let elapsed = self.start.elapsed();
            eprintln!("[total] {:.1}ms", elapsed.as_secs_f64() * 1000.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_disables_the_deadline_for_the_timer_and_the_watchdog() {
        assert_eq!(deadline(0), None);
        assert!(!expired(Duration::from_secs(3600), deadline(0)));
        assert!(!expired(Duration::from_millis(999), deadline(1)));
        assert!(expired(Duration::from_millis(1001), deadline(1)));
    }

    #[test]
    fn a_timeout_is_one_diagnostics_document_in_json() {
        let diagnostic = timeout_diagnostic(3, Some(3.04), Some("check"));
        assert_eq!(diagnostic.code.0, "FRS-COMP-0008");
        assert_eq!(
            diagnostic.message.as_ref(),
            "compilation timeout (3.0s > 3s limit) after phase 'check'"
        );
        let mut bundle = DiagnosticBundle::new();
        bundle.push(diagnostic);
        let json: serde_json::Value = serde_json::from_str(
            &format_diagnostics_json_with_verbosity(&bundle, ErrorVerbosity::Standard),
        )
        .expect("one JSON document");
        assert_eq!(json["schema_version"], 2);
        assert_eq!(json["status"], "failed");
        assert_eq!(json["diagnostics"][0]["code"], "FRS-COMP-0008");
        // the watchdog has no phase
        assert_eq!(
            timeout_diagnostic(5, None, None).message.as_ref(),
            "compilation timeout (5s limit exceeded)"
        );
    }
}
