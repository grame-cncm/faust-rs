//! `--compare`, `--ref` and `--check`: did this change a sample?
//!
//! Two halves. [`Verification::prepare`] settles everything before any render,
//! like the rest of the command line: the tolerance, the checks, the second
//! program compiled and its own writes checked, the reference file read.
//! [`Verification::judge`] then looks at one render's samples and returns
//! what is said of them and whether the command fails.

use std::rc::Rc;

use cranelift_ffi::probe::audio_file::read_channels;
use cranelift_ffi::probe::compare::{Comparison, Samples, Tolerance, compare};
use cranelift_ffi::probe::engine::{Factory, Probe, RenderSpec};
use cranelift_ffi::probe::number::NumberFormat;
use cranelift_ffi::probe::schedule::Schedule;

use crate::cli::{Args, Format, refuse, verification_requested};
use crate::determinism;
use crate::report::{json_number, non_finite_name};
use crate::setup::{assignments, compile_program, compiler_args};
use crate::writes::{Clamped, check_value};

/// An invariant `--check` verifies on the render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Check {
    /// The same samples at this block size.
    Block(usize),
    /// The same samples again after a reset of the instance.
    Reset,
    /// The same samples from a second compilation.
    Determinism,
    /// The distance between the two sample widths.
    Width,
}

impl Check {
    pub(crate) fn name(self) -> String {
        match self {
            Self::Block(size) => format!("block={size}"),
            Self::Reset => "reset".to_owned(),
            Self::Determinism => "determinism".to_owned(),
            Self::Width => "width".to_owned(),
        }
    }
}

/// Block sizes `--check block` tries when none is given: a sample at a time,
/// a size that divides nothing, and a large one.
pub(crate) const DEFAULT_CHECK_BLOCKS: [usize; 3] = [1, 7, 512];

/// Parses the `--check` occurrences. `block` sizes equal to the render's own
/// are dropped: that render is what the others are compared with.
pub(crate) fn parse_checks(specs: &[String], own_block: usize) -> Result<Vec<Check>, String> {
    let mut checks = Vec::new();
    let mut push = |check: Check| {
        if check != Check::Block(own_block) && !checks.contains(&check) {
            checks.push(check);
        }
    };
    for spec in specs {
        match spec.as_str() {
            "reset" => push(Check::Reset),
            "determinism" => push(Check::Determinism),
            "width" => push(Check::Width),
            "block" => DEFAULT_CHECK_BLOCKS
                .into_iter()
                .for_each(|n| push(Check::Block(n))),
            "all" => {
                DEFAULT_CHECK_BLOCKS
                    .into_iter()
                    .for_each(|n| push(Check::Block(n)));
                push(Check::Reset);
                push(Check::Determinism);
                push(Check::Width);
            }
            other => {
                let sizes = other.strip_prefix("block=").ok_or_else(|| {
                    format!(
                        "unknown check `{other}` (expected block[=N1,N2,...], reset, determinism, width or all)"
                    )
                })?;
                for size in sizes.split(',') {
                    match size.trim().parse::<usize>() {
                        Ok(n) if n > 0 => push(Check::Block(n)),
                        _ => {
                            return Err(format!("`--check {other}`: `{size}` is not a block size"));
                        }
                    }
                }
            }
        }
    }
    Ok(checks)
}

/// Renders `probe` from a cleared instance with `sets` written, keeping the
/// window's samples. A reference that is not finite cannot be compared with.
pub(crate) fn render_for_comparison(
    probe: &Probe,
    spec: &RenderSpec,
    sets: &[(&str, f64)],
    what: &str,
) -> Result<Samples, String> {
    probe.reset();
    for (path, value) in sets {
        probe.set(path, *value)?;
    }
    let (stats, samples) = probe.collect(spec);
    if let Some((channel, located)) = stats.first_non_finite() {
        return Err(format!(
            "{what} produced non-finite samples: first at frame {}, out{channel} ({})",
            located.frame,
            non_finite_name(located.value)
        ));
    }
    Ok(samples)
}

/// One comparison, as the lines printed with the statistics. `gate` says
/// whether a disagreement fails the command or is only reported (`--check
/// width` without a tolerance).
pub(crate) fn comparison_lines(
    tag: &str,
    comparison: &Comparison,
    gate: bool,
    fmt: &NumberFormat,
) -> Vec<String> {
    comparison
        .channels
        .iter()
        .map(|(ch, diff)| {
            if diff.identical {
                return format!("# {tag} out{ch}: identical");
            }
            let at = diff.max_abs_at.map_or_else(
                || "a non-finite sample".to_owned(),
                |frame| format!("frame {frame}"),
            );
            let verdict = match diff.first_beyond {
                Some(d) if gate => format!(
                    "first beyond tolerance: frame {} ({} vs {})",
                    d.frame,
                    fmt.sample(d.value),
                    fmt.sample(d.reference)
                ),
                Some(d) => format!("first difference: frame {}", d.frame),
                None => "within tolerance".to_owned(),
            };
            format!(
                "# {tag} out{ch}: max_abs={} at {at}, max_rel={}, {verdict}",
                fmt.computed(diff.max_abs),
                fmt.computed(diff.max_rel)
            )
        })
        .collect()
}

/// One comparison, for the JSON document.
pub(crate) fn comparison_json(comparison: &Comparison) -> serde_json::Value {
    let channels: Vec<serde_json::Value> = comparison
        .channels
        .iter()
        .map(|(ch, diff)| {
            serde_json::json!({
                "output": ch,
                "identical": diff.identical,
                "max_abs": json_number(diff.max_abs),
                "max_abs_at": diff.max_abs_at,
                "max_rel": json_number(diff.max_rel),
                "first_beyond": diff.first_beyond.map(|d| serde_json::json!({
                    "frame": d.frame,
                    "value": json_number(d.value),
                    "reference": json_number(d.reference),
                })),
            })
        })
        .collect();
    serde_json::json!({ "agrees": comparison.agrees(), "channels": channels })
}

/// What `--compare`, `--ref` and `--check` need, settled before any render.
pub(crate) struct Verification<'a> {
    /// Whether any of the three was given.
    pub(crate) requested: bool,
    tolerance: Tolerance,
    /// Whether `--tolerance` or `--rel-tolerance` was given: `--check width`
    /// is a gate with one, a report without.
    explicit_tolerance: bool,
    compared_outputs: Option<&'a [usize]>,
    checks: Vec<Check>,
    /// `--compare`: the second program, and the values it alone is given.
    other: Option<(&'a str, Probe)>,
    other_sets: Vec<(&'a str, f64)>,
    /// `--ref`: the render saved earlier.
    reference: Option<(&'a str, Samples)>,
}

/// The render that is judged, and what rendering it again takes.
pub(crate) struct Subject<'a> {
    pub(crate) args: &'a Args,
    pub(crate) factory: &'a Rc<Factory>,
    pub(crate) probe: &'a Probe,
    pub(crate) spec: &'a RenderSpec,
    /// The `--set` values of FILE.
    pub(crate) fixed: &'a [(&'a str, f64)],
    pub(crate) fmt: &'a NumberFormat,
}

/// What is said of a render that was verified.
#[derive(Default)]
pub(crate) struct Verdict {
    /// The `# compare` and `# check` lines.
    pub(crate) lines: Vec<String>,
    /// Their JSON counterparts, `compare` and `checks`.
    pub(crate) json: serde_json::Map<String, serde_json::Value>,
    /// The first comparison or check that failed: reported after the output,
    /// which carries its details.
    pub(crate) failure: Option<String>,
}

impl<'a> Verification<'a> {
    /// Validates the three flags and what they name. `clamped` receives the
    /// clamps of the second program's writes, which every render runs under.
    pub(crate) fn prepare(
        args: &'a Args,
        probe: &Probe,
        schedule: &Schedule,
        sweeping: bool,
        clamped: &mut Vec<Clamped>,
    ) -> Result<Self, String> {
        let requested = verification_requested(args);
        if requested {
            refuse(
                &[
                    ("--sweep", sweeping),
                    ("--format ir", args.format == Format::Ir),
                ],
                |flag| {
                    format!(
                        "{flag} cannot be combined with --compare, --ref or --check, which look at one render"
                    )
                },
            )?;
        }
        if args.compare.is_some() && args.reference.is_some() {
            return Err("--compare and --ref both name the reference: give one".to_owned());
        }
        if args.compare.is_none() && (!args.set_a.is_empty() || !args.set_b.is_empty()) {
            return Err("--set-a and --set-b address the two programs of --compare".to_owned());
        }
        for (flag, value) in [
            ("--tolerance", args.tolerance),
            ("--rel-tolerance", args.rel_tolerance),
        ] {
            if value.is_some_and(|v| !(v >= 0.0 && v.is_finite())) {
                return Err(format!("{flag} must be a non-negative number"));
            }
        }
        if let Some(&beyond) = args
            .compare_outputs
            .iter()
            .find(|&&ch| ch >= probe.outputs())
        {
            return Err(format!(
                "--compare-outputs {beyond}: the program has {} output(s)",
                probe.outputs()
            ));
        }
        let checks = parse_checks(&args.checks, args.block)?;
        // the second program, with the values it alone is given
        let other_sets = assignments(args.sets.iter().chain(&args.set_b))?;
        let other = match &args.compare {
            Some(path) => Some((
                path.as_str(),
                other_program(args, path, probe, &other_sets, schedule, clamped)?,
            )),
            None => None,
        };
        let reference = match &args.reference {
            Some(path) => {
                let (channels, _) = read_channels(std::path::Path::new(path))?;
                Some((
                    path.as_str(),
                    Samples {
                        start: args.skip,
                        channels,
                    },
                ))
            }
            None => None,
        };
        Ok(Self {
            requested,
            tolerance: Tolerance {
                abs: args.tolerance.unwrap_or(0.0),
                rel: args.rel_tolerance.unwrap_or(0.0),
            },
            explicit_tolerance: args.tolerance.is_some() || args.rel_tolerance.is_some(),
            compared_outputs: (!args.compare_outputs.is_empty())
                .then_some(args.compare_outputs.as_slice()),
            checks,
            other,
            other_sets,
            reference,
        })
    }

    /// Judges one render. `context` explains a frame, as for a failed render.
    pub(crate) fn judge(
        &self,
        subject: &Subject<'_>,
        render: &Samples,
        context: impl Fn(usize) -> String,
    ) -> Result<Verdict, String> {
        let mut verdict = Verdict::default();
        if !self.requested {
            return Ok(verdict);
        }
        let fmt = subject.fmt;
        let mut failure: Option<String> = None;
        let mut failed = |what: String, comparison: &Comparison| {
            if failure.is_none()
                && let Some((channel, d)) = comparison.first_beyond()
            {
                failure = Some(format!(
                    "{what}\n  first: frame {}, out{channel}: {} vs {}{}",
                    d.frame,
                    fmt.sample(d.value),
                    fmt.sample(d.reference),
                    context(d.frame)
                ));
            }
        };
        let reference = match (&self.other, &self.reference) {
            (Some((path, other)), _) => Some((
                *path,
                render_for_comparison(other, subject.spec, &self.other_sets, &format!("`{path}`"))?,
            )),
            (None, Some((path, samples))) => Some((*path, samples.clone())),
            (None, None) => None,
        };
        if let Some((name, reference)) = reference {
            let comparison = compare(render, &reference, self.tolerance, self.compared_outputs)
                .map_err(|error| format!("cannot compare with `{name}`: {error}"))?;
            verdict.lines.push(format!(
                "# compare: against {name}, tolerance abs={} rel={}",
                self.tolerance.abs, self.tolerance.rel
            ));
            verdict
                .lines
                .extend(comparison_lines("compare", &comparison, true, fmt));
            let mut json = comparison_json(&comparison);
            json["reference"] = serde_json::json!(name);
            verdict.json.insert("compare".to_owned(), json);
            if !comparison.agrees() {
                failed(
                    format!("the render differs from `{name}` beyond the tolerance"),
                    &comparison,
                );
            }
        }
        let mut checks_json = Vec::new();
        for check in &self.checks {
            let tag = format!("check {}", check.name());
            let (samples, gate, limit) =
                self.render_again(*check, &tag, subject, &mut verdict.lines)?;
            let comparison = compare(render, &samples, limit, self.compared_outputs)
                .map_err(|error| format!("{tag}: {error}"))?;
            verdict
                .lines
                .extend(comparison_lines(&tag, &comparison, gate, fmt));
            let mut json = comparison_json(&comparison);
            json["check"] = serde_json::json!(check.name());
            json["gate"] = serde_json::json!(gate);
            checks_json.push(json);
            if gate && !comparison.agrees() {
                failed(
                    format!("--{tag} failed: the render is not the same"),
                    &comparison,
                );
            }
        }
        if !checks_json.is_empty() {
            verdict
                .json
                .insert("checks".to_owned(), serde_json::Value::Array(checks_json));
        }
        verdict.failure = failure;
        Ok(verdict)
    }

    /// The second render a check compares the first with: its samples,
    /// whether a disagreement fails the command, and within what tolerance.
    fn render_again(
        &self,
        check: Check,
        tag: &str,
        subject: &Subject<'_>,
        lines: &mut Vec<String>,
    ) -> Result<(Samples, bool, Tolerance), String> {
        let Subject {
            args,
            factory,
            probe,
            spec,
            fixed,
            ..
        } = subject;
        Ok(match check {
            Check::Block(size) => {
                let fresh = Probe::instantiate(factory, args.sr)?;
                let at_size = RenderSpec {
                    block: size,
                    ..(*spec).clone()
                };
                let samples = render_for_comparison(&fresh, &at_size, fixed, tag)?;
                (samples, true, self.tolerance)
            }
            Check::Reset => (
                render_for_comparison(probe, spec, fixed, tag)?,
                true,
                self.tolerance,
            ),
            Check::Determinism => {
                let (key, samples) = determinism::render(args)?;
                let same_key = key == factory.sha_key();
                lines.push(format!(
                    "# {tag}: a second compilation gives {} program key",
                    if same_key { "the same" } else { "ANOTHER" }
                ));
                lines.push(format!("# {tag}: rendered in an independent process"));
                // two compilations of one source owe each other the very bits
                (samples, true, Tolerance::default())
            }
            Check::Width => {
                let (other_width, _) = compile_program(args, !args.compile.double)?;
                let fresh = Probe::instantiate(&Rc::new(other_width), args.sr)?;
                lines.push(format!(
                    "# {tag}: this render in {} precision against the {} one{}",
                    if args.compile.double {
                        "double"
                    } else {
                        "single"
                    },
                    if args.compile.double {
                        "single"
                    } else {
                        "double"
                    },
                    if self.explicit_tolerance {
                        ""
                    } else {
                        " (a report: no tolerance given)"
                    }
                ));
                (
                    render_for_comparison(&fresh, spec, fixed, tag)?,
                    self.explicit_tolerance,
                    self.tolerance,
                )
            }
        })
    }
}

/// The second program of `--compare`: compiled, of the same arity as the
/// first, and what is written to it checked as for the first.
fn other_program(
    args: &Args,
    path: &str,
    probe: &Probe,
    other_sets: &[(&str, f64)],
    schedule: &Schedule,
    clamped: &mut Vec<Clamped>,
) -> Result<Probe, String> {
    let other_factory = Factory::compile_with_args(
        path,
        &args.import_dirs,
        &compiler_args(args),
        args.compile.double,
        args.opt_level,
    )
    .map_err(|error| format!("--compare: {error}"))?;
    let other = Probe::instantiate(&Rc::new(other_factory), args.sr)?;
    if other.outputs() != probe.outputs() {
        return Err(format!(
            "--compare: `{}` has {} output(s) and `{path}` {}",
            args.file,
            probe.outputs(),
            other.outputs()
        ));
    }
    for (query, value) in other_sets {
        check_value(other.controls(), query, *value, args.clamp, clamped)
            .map_err(|error| format!("--compare `{path}`: {error}"))?;
    }
    for (_, query, value) in schedule.param_writes() {
        check_value(other.controls(), query, value, args.clamp, clamped)
            .map_err(|error| format!("--compare `{path}`: {error}"))?;
    }
    Ok(other)
}
