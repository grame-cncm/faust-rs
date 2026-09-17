//! `--train` and `--fd-check`: the host loop of a differentiable program.
//!
//! [`run_train`] is the sequence: the writes checked, the grid when `--sweep`
//! asks for one, the gradient checked at the start, the descent, its summary,
//! the gradient checked at the end. Each is a function below, and [`Report`]
//! is what they all say through.

use std::rc::Rc;

use cranelift_ffi::probe::engine::{Factory, Probe};
use cranelift_ffi::probe::number::NumberFormat;
use cranelift_ffi::probe::timing::BlockTimer;
use cranelift_ffi::probe::train::{self, BoundStats, FdCheck, Optimizer, TrainSpec, Trained};

use crate::cli::{Args, FdWhere, Format, OptimizerKind, Protocol, refuse, verification_requested};
use crate::report::{document, json_number, print_json, three_digits, time_lines, timing_json};
use crate::setup::{
    assignments, compile_timed, number_format, parse_assignment, parse_input_at, sweep_axes,
};
use crate::writes::{Clamped, check_value};

/// What the projection onto its range did to a trained control, for the
/// `# trained` line: nothing is said of a control that never met a bound.
pub(crate) fn bound_text(stats: &BoundStats, blocks: usize) -> Option<String> {
    let parts: Vec<String> = [
        (train::Bound::Lower, stats.on_lower),
        (train::Bound::Upper, stats.on_upper),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(bound, count)| {
        format!(
            "on its {} bound for {count} of {blocks} blocks, {}",
            bound.name(),
            if stats.ends_on == Some(bound) {
                "the last one included"
            } else {
                "not the last one"
            }
        )
    })
    .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// One `--fd-check`: its lines (none under `--format json`), its JSON, and
/// the error when a lane departs from the finite differences. `place` is
/// `start` or `end`; the lines of `start` are those the tool always printed.
pub(crate) fn fd_check_report(
    args: &Args,
    place: &str,
    checks: &[FdCheck],
) -> (Vec<String>, serde_json::Value, Option<String>) {
    let tag = if place == "start" {
        String::new()
    } else {
        format!(" {place}")
    };
    let mut worst = 0.0_f64;
    let mut lines = Vec::new();
    for check in checks {
        // at the end of a descent the gradient is small, which is the point:
        // six decimals would print it as zero
        lines.push(if place == "start" {
            format!(
                "# fd-check {}: rad {:.6} fd {:.6} relative error {:.2e}",
                check.path, check.rad, check.fd, check.relative_error
            )
        } else {
            format!(
                "# fd-check{tag} {}: rad {:.6e} fd {:.6e} relative error {:.2e}",
                check.path, check.rad, check.fd, check.relative_error
            )
        });
        // a NaN error is the worst there is, and `max` would drop it
        worst = if check.relative_error.is_nan() || worst.is_nan() {
            f64::NAN
        } else {
            worst.max(check.relative_error)
        };
    }
    lines.push(format!(
        "# fd-check{tag}: block {} frames, step {}, worst relative error {worst:.2e} (tolerance {})",
        args.block, args.fd_step, args.fd_tolerance
    ));
    let passes = worst <= args.fd_tolerance;
    let json = serde_json::json!({
        "block": args.block,
        "step": json_number(args.fd_step),
        "tolerance": json_number(args.fd_tolerance),
        "worst_relative_error": json_number(worst),
        "passes": passes,
        "checks": checks.iter().map(|check| serde_json::json!({
            "path": check.path,
            "rad": json_number(check.rad),
            "fd": json_number(check.fd),
            "fd_plain": json_number(check.fd_plain),
            "relative_error": json_number(check.relative_error),
        })).collect::<Vec<_>>(),
    });
    let failure = (!passes).then(|| {
        format!(
            "a gradient lane departs from finite differences by {worst:.2e} at the {place} of the descent, above --fd-tolerance {}",
            args.fd_tolerance
        )
    });
    (lines, json, failure)
}

/// What a host loop does not take.
fn refuse_flags(args: &Args) -> Result<(), String> {
    refuse(
        &[
            ("--reduce", args.reduce.is_some()),
            ("--at", !args.ats.is_empty()),
            ("--bargraphs", args.bargraphs),
            (
                "--protocol impulse-test",
                args.protocol == Protocol::ImpulseTest,
            ),
            ("--format ir", args.format == Format::Ir),
            ("--out", args.out.is_some()),
            ("--fail-above", args.fail_above.is_some()),
            ("--compare/--ref/--check", verification_requested(args)),
            ("--freqresp", args.freqresp.is_some()),
        ],
        |flag| format!("{flag} cannot be combined with --train / --fd-check"),
    )
}

/// What a host loop says. The lines are the output as it comes, a descent
/// being watched; the JSON document is printed once, at the end or with the
/// failure.
struct Report<'a> {
    args: &'a Args,
    as_json: bool,
    train: serde_json::Map<String, serde_json::Value>,
}

impl<'a> Report<'a> {
    fn new(args: &'a Args) -> Self {
        Self {
            args,
            as_json: args.format == Format::Json,
            train: serde_json::Map::new(),
        }
    }

    fn say(&self, line: impl AsRef<str>) {
        if !self.as_json {
            println!("{}", line.as_ref());
        }
    }

    fn say_all<L: AsRef<str>>(&self, lines: impl IntoIterator<Item = L>) {
        for line in lines {
            self.say(line);
        }
    }

    fn insert(&mut self, key: &str, value: serde_json::Value) {
        self.train.insert(key.to_owned(), value);
    }

    /// Prints the document, under `--format json`.
    fn finish(self) -> Result<(), String> {
        if !self.as_json {
            return Ok(());
        }
        let mut document = document(self.args);
        document.insert("train".to_owned(), serde_json::Value::Object(self.train));
        print_json(document)
    }
}

/// `--train` and `--fd-check`: the host loop of a program whose loss and
/// gradient lanes leave the graph (see `probe::train`).
pub(crate) fn run_train(args: &Args) -> Result<(), String> {
    if args.train.is_empty() {
        return Err("--fd-check needs the controls to check: --train CONTROLS".to_owned());
    }
    refuse_flags(args)?;
    let fd_at_start = matches!(args.fd_check, Some(FdWhere::Start | FdWhere::Both));
    let fd_at_end = matches!(args.fd_check, Some(FdWhere::End | FdWhere::Both));
    if fd_at_end && args.blocks == 0 {
        return Err(
            "--fd-check=end checks the trained values: it needs a descent, --blocks > 0".to_owned(),
        );
    }
    let fmt = number_format(args)?;
    let compiled = compile_timed(args)?;
    let factory = &compiled.factory;
    let mut report = Report::new(args);

    let (axes, inputs) = check_writes(args, factory, &mut report)?;
    let mut spec = TrainSpec {
        params: args.train.clone(),
        loss_lane: args.loss_lane,
        first_grad_lane: args.grad_lane,
        optimizer: match args.optimizer {
            OptimizerKind::Adam => Optimizer::ADAM,
            OptimizerKind::Sgd => Optimizer::Sgd,
        },
        lr: args.lr,
        block: args.block,
        blocks: args.blocks,
        input: parse_input_at(&args.input, args.sr, inputs)?,
        reset_per_block: args.reset_per_block,
        sets: assignments(&args.sets)?
            .into_iter()
            .map(|(path, value)| (path.to_owned(), value))
            .collect(),
    };
    report.insert(
        "options",
        serde_json::json!({
            "optimizer": match args.optimizer {
                OptimizerKind::Adam => "adam",
                OptimizerKind::Sgd => "sgd",
            },
            "lr": json_number(args.lr),
            "block": args.block,
            "blocks": args.blocks,
            "reset_per_block": args.reset_per_block,
        }),
    );
    if !axes.is_empty() {
        grid_then_start(args, factory, &mut spec, &axes, &fmt, &mut report)?;
    }

    let mut fd_reports = serde_json::Map::new();
    if fd_at_start {
        let checks = train::fd_check(factory, args.sr, &spec, args.fd_step, None)?;
        let (lines, json, failure) = fd_check_report(args, "start", &checks);
        report.say_all(lines);
        fd_reports.insert("start".to_owned(), json);
        if let Some(failure) = failure {
            report.insert("fd_check", serde_json::Value::Object(fd_reports));
            report.finish()?;
            return Err(failure);
        }
    }
    if args.blocks == 0 {
        if !fd_reports.is_empty() {
            report.insert("fd_check", serde_json::Value::Object(fd_reports));
        }
        if args.time {
            let none = BlockTimer::new(f64::from(args.sr)).finish();
            report.say_all(time_lines(compiled.seconds, &none, |_| String::new()));
            report.insert(
                "timing",
                serde_json::json!({ "compile_s": json_number(compiled.seconds) }),
            );
        }
        return report.finish();
    }

    let (rows, outcome) = descend(args, factory, &spec, &fmt, report.as_json);
    report.insert("rows", serde_json::Value::Array(rows));
    let trained = match outcome {
        Ok(trained) => trained,
        Err(error) => {
            // the rows up to the block that failed are the evidence
            report.finish()?;
            return Err(error);
        }
    };
    summarize(args, &trained, &fmt, &mut report);

    // A descent is finished where the gradient is small *and* right: the
    // same check, at the trained values.
    let mut failure = None;
    if fd_at_end {
        let checks = train::fd_check(factory, args.sr, &spec, args.fd_step, Some(&trained.values))?;
        let (lines, json, failed) = fd_check_report(args, "end", &checks);
        report.say_all(lines);
        fd_reports.insert("end".to_owned(), json);
        failure = failed;
    }
    if !fd_reports.is_empty() {
        report.insert("fd_check", serde_json::Value::Object(fd_reports));
    }
    if args.time {
        report.say_all(time_lines(compiled.seconds, &trained.timing, |worst| {
            format!("block {}", worst.frame / args.block.max(1) + 1)
        }));
        let mut timing = timing_json(&trained.timing);
        timing["compile_s"] = json_number(compiled.seconds);
        report.insert("timing", timing);
    }
    report.finish()?;
    failure.map_or(Ok(()), Err)
}

/// The axes of a grid of starting points: a control, and the values it takes.
type GridAxes = Vec<(String, Vec<f64>)>;

/// Checks `--set` and the values of `--sweep` against their ranges, says the
/// clamps, and returns the sweep's axes with the values the grid will use,
/// and the program's number of inputs.
///
/// A `--set` outside its range: on a trained control it would move the
/// starting point, on another it would fix a value that is not the one asked
/// for, and the descent's rows would show neither. The values of a `--sweep`
/// are starting points and are checked alike.
fn check_writes(
    args: &Args,
    factory: &Rc<Factory>,
    report: &mut Report<'_>,
) -> Result<(GridAxes, usize), String> {
    let mut clamped = Vec::new();
    let mut axes: GridAxes = Vec::new();
    let probe = Probe::instantiate(factory, args.sr)?;
    // one at a time: of two faulty `--set`, the first is the one reported
    for assignment in &args.sets {
        let (path, value) = parse_assignment(assignment)?;
        check_value(probe.controls(), path, value, args.clamp, &mut clamped)?;
    }
    for axis in sweep_axes(args)? {
        let applied = axis
            .values
            .iter()
            .map(|value| {
                check_value(
                    probe.controls(),
                    &axis.path,
                    *value,
                    args.clamp,
                    &mut clamped,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        axes.push((axis.path, applied));
    }
    report.say_all(clamped.iter().map(Clamped::line));
    if !clamped.is_empty() {
        report.insert(
            "clamped",
            serde_json::Value::Array(clamped.iter().map(Clamped::json).collect()),
        );
    }
    Ok((axes, probe.inputs()))
}

/// Grid, then descent: `--sweep` over trained controls gives the loss of one
/// block at every point, and the descent leaves from the best. What a
/// non-convex loss needs, and what took a sweep, a parse and a second command.
fn grid_then_start(
    args: &Args,
    factory: &Rc<Factory>,
    spec: &mut TrainSpec,
    axes: &GridAxes,
    fmt: &NumberFormat,
    report: &mut Report<'_>,
) -> Result<(), String> {
    let grid = train::grid(factory, args.sr, spec, axes)?;
    let mut points = Vec::new();
    for (index, (values, loss)) in grid.points.iter().enumerate() {
        let at: Vec<String> = grid
            .paths
            .iter()
            .zip(values)
            .map(|(path, value)| format!("{path}={value}"))
            .collect();
        report.say(format!(
            "# grid {} loss={}{}",
            at.join(" "),
            fmt.loss(*loss),
            if index == grid.best { " (best)" } else { "" }
        ));
        let mut set = serde_json::Map::new();
        for (path, value) in grid.paths.iter().zip(values) {
            set.insert(path.clone(), json_number(*value));
        }
        points.push(serde_json::json!({ "set": set, "loss": json_number(*loss) }));
    }
    report.say(format!(
        "# grid: {} points, one block of {} frames each; the descent starts from the best",
        grid.points.len(),
        args.block
    ));
    report.insert(
        "grid",
        serde_json::json!({ "points": points, "best": grid.best }),
    );
    // after the `--set` values: on a control given both, the grid decides
    spec.sets.extend(grid.best_assignments());
    Ok(())
}

/// The descent, its rows printed as they come (every `--every` blocks, and
/// the last), or kept for the JSON document.
fn descend(
    args: &Args,
    factory: &Rc<Factory>,
    spec: &TrainSpec,
    fmt: &NumberFormat,
    as_json: bool,
) -> (Vec<serde_json::Value>, Result<Trained, String>) {
    let mut header_done = false;
    let mut rows = Vec::new();
    let every = args.every.max(1);
    let outcome = train::train(factory, args.sr, spec, |step| {
        if step.block % every != 0 && step.block != args.blocks {
            return;
        }
        if as_json {
            rows.push(serde_json::json!({
                "block": step.block,
                "loss": json_number(step.loss),
                "values": step.params.iter().map(|v| json_number(*v)).collect::<Vec<_>>(),
                "grads": step.grads.iter().map(|g| json_number(*g)).collect::<Vec<_>>(),
            }));
            return;
        }
        if !header_done {
            let mut header = format!("block,loss,{}", spec.params.join(","));
            if args.train_verbose {
                for param in &spec.params {
                    header.push_str(&format!(",grad_{param}"));
                }
            }
            println!("{header}");
            header_done = true;
        }
        let mut fields: Vec<String> = step.params.iter().map(|v| fmt.computed(*v)).collect();
        if args.train_verbose {
            fields.extend(step.grads.iter().map(|g| fmt.loss(*g)));
        }
        println!(
            "{},{},{}",
            step.block,
            fmt.loss(step.loss),
            fields.join(",")
        );
    });
    (rows, outcome)
}

/// What is said of a finished descent: where each control ended and what its
/// range did to it, the loss, and the facts a reader would otherwise miss.
fn summarize(args: &Args, trained: &Trained, fmt: &NumberFormat, report: &mut Report<'_>) {
    let mut notes = Vec::new();
    let mut trained_json = Vec::new();
    let mut stopped = Vec::new();
    for (k, path) in trained.paths.iter().enumerate() {
        let bounds = &trained.bounds[k];
        let bound =
            bound_text(bounds, args.blocks).map_or_else(String::new, |text| format!(" ({text})"));
        report.say(format!(
            "# trained {path}={}{bound}",
            fmt.computed(trained.values[k])
        ));
        if bounds.ends_on.is_some() {
            stopped.push(path.as_str());
        }
        trained_json.push(serde_json::json!({
            "path": path,
            "value": json_number(trained.values[k]),
            "min": json_number(trained.ranges[k].0),
            "max": json_number(trained.ranges[k].1),
            "blocks_on_lower": bounds.on_lower,
            "blocks_on_upper": bounds.on_upper,
            "ends_on": bounds.ends_on.map(train::Bound::name),
        }));
    }
    report.say(format!(
        "# loss: block 1 {:.6e}, block {} {:.6e}",
        trained.first_loss, args.blocks, trained.last_loss
    ));
    report.say(format!(
        "# loss: minimum {:.6e} at block {}",
        trained.min_loss, trained.min_loss_block
    ));
    if !stopped.is_empty() {
        notes.push(format!(
            "a control that ends on a bound is stopped, not converged: {}",
            stopped.join(" ")
        ));
    }
    // a ratio means something for a positive loss only
    if trained.min_loss > 0.0 && trained.last_loss > 10.0 * trained.min_loss {
        let then: Vec<String> = trained
            .paths
            .iter()
            .zip(&trained.values_at_min)
            .map(|(path, value)| format!("{path}={}", fmt.computed(*value)))
            .collect();
        notes.push(format!(
            "the last loss is {} times the minimum, which block {} reached with {}",
            three_digits(trained.last_loss / trained.min_loss),
            trained.min_loss_block,
            then.join(" ")
        ));
    }
    report.say_all(notes.iter().map(|note| format!("# note: {note}")));
    report.insert("trained", serde_json::Value::Array(trained_json));
    report.insert(
        "loss",
        serde_json::json!({
            "first": json_number(trained.first_loss),
            "last": json_number(trained.last_loss),
            "min": json_number(trained.min_loss),
            "min_block": trained.min_loss_block,
            "values_at_min": trained.values_at_min.iter().map(|v| json_number(*v)).collect::<Vec<_>>(),
        }),
    );
    if !notes.is_empty() {
        report.insert("notes", serde_json::json!(notes));
    }
}
