//! What `--train` says about the descent it ran (phase F4 of
//! `porting/faustprobe-feedback-quality-analysis-and-plan-2026-09-17-en.md`).
//!
//! The loss and the controls were all a descent printed. What was learned by
//! reading tables afterwards is now said by the loop: a control that ends on
//! a bound is stopped, not converged; a gradient that vanishes is not a step
//! that vanishes; the last loss is not the best one; a gradient checked where
//! the descent starts may be wrong where it stops; and a non-convex loss wants
//! a grid before the descent, in one command.
//!
//! The fixtures carry their own gradient lane, written by hand: the host loop
//! needs a loss and its gradient, not `rad`, and a closed form is what lets
//! every expected number below be replayed in the test.

mod common;
use common::{Fixtures, json, probe, probe_source};

/// The minimum of `(x - 3)^2` is outside `[0, 2]`; that of `(y - 0.5)^2` is
/// inside `[-1, 1]`.
const BOUND: &str = "x = hslider(\"x\", 1, 0, 2, 0.001);\n\
y = hslider(\"y\", 0, -1, 1, 0.001);\n\
process = (x - 3) * (x - 3) + (y - 0.5) * (y - 0.5), 2 * (x - 3), 2 * (y - 0.5);\n";

/// A target out of reach (3) for three blocks of 64 frames, then one inside
/// the range (1): the control meets its bound and leaves it. `n` counts
/// 1, 2, ... so `n > 192` first holds at frame 192, the first of block 4.
const LEAVES: &str = "n = +(1) ~ _;\n\
t = select2(n > 192, 3, 1);\n\
x = hslider(\"x\", 1, 0, 2, 0.001);\n\
process = (x - t) * (x - t), 2 * (x - t);\n";

/// A target the descent converges to (0.5) for five blocks, then another
/// (1.5): the last loss is far above the best one.
const JUMPS: &str = "n = +(1) ~ _;\n\
t = select2(n > 320, 0.5, 1.5);\n\
x = hslider(\"x\", 1, 0, 2, 0.001);\n\
process = (x - t) * (x - t), 2 * (x - t);\n";

/// The same, with a second target (0.6) close to the first: the last loss is
/// above the best one, and not by much.
const JUMPS_A_LITTLE: &str = "n = +(1) ~ _;\n\
t = select2(n > 320, 0.5, 0.6);\n\
x = hslider(\"x\", 1, 0, 2, 0.001);\n\
process = (x - t) * (x - t), 2 * (x - t);\n";

/// `(x^2 - 1)^2 + 0.3 x`: a local minimum near 0.96, where a descent from
/// the slider's 1 stops, and the global one near -1.04.
const WELLS: &str = "x = hslider(\"x\", 1, -2, 2, 0.001);\n\
process = (x * x - 1) * (x * x - 1) + 0.3 * x, 4 * x * (x * x - 1) + 0.3;\n";

/// A gradient lane that is right below 1.5 and four times too small above:
/// right where the descent starts, wrong where it stops.
const WRONG_LATER: &str = "x = hslider(\"x\", 1, 0, 2, 0.001);\n\
process = (x - 3) * (x - 3), select2(x > 1.5, 2 * (x - 3), 0.5 * (x - 3));\n";

/// `exp(a u) - a u - 1` with `u = x - 0.5` and `a = 50`: its minimum is at the
/// slider's initial value, where the gradient `a (exp(a u) - 1)` is exactly
/// zero and the third derivative, `a^3`, is large.
const CURVED: &str = "u = hslider(\"x\", 0.5, 0, 1, 0.001) - 0.5;\n\
process = exp(50 * u) - 50 * u - 1, 50 * (exp(50 * u) - 1);\n";

/// A loss that depends on a control the descent does not move.
const OFFSET: &str = "x = hslider(\"x\", 0, -2, 2, 0.001);\n\
c = hslider(\"c\", 0, -2, 2, 0.001);\n\
process = (x - c) * (x - c), 2 * (x - c);\n";

/// A loss that is infinite at the slider's initial value.
const POLE: &str = "x = hslider(\"x\", 1, 0, 2, 0.001);\n\
process = 1 / (x - 1), 0 * x;\n";

/// The loss is the control: the first block's loss is the starting point.
const IDENTITY: &str = "x = hslider(\"x\", 0.1, 0, 0.7, 0.001);\nprocess = x, 1 + 0 * x;\n";

/// A descent by plain gradient descent on `file`, in double precision, with
/// no input: the arguments every test below shares.
fn descend<'a>(file: &'a str, train: &'a str, lr: &'a str, blocks: &'a str) -> Vec<&'a str> {
    vec![
        "--double",
        "--in",
        "zero",
        "--block",
        "64",
        "--optimizer",
        "sgd",
        "--lr",
        lr,
        "--blocks",
        blocks,
        "--every",
        "1",
        "--train",
        train,
        file,
    ]
}

fn lines_of<'a>(stdout: &'a str, prefix: &str) -> Vec<&'a str> {
    stdout.lines().filter(|l| l.starts_with(prefix)).collect()
}

// ------------------------------------------------------------------ bounds

/// `x <- x + 0.2 (3 - x)` from 1: 1.4, 1.72, 1.976, then 2.2288, which the
/// range cuts to 2. The test replays the recurrence and counts.
#[test]
fn a_control_that_ends_on_a_bound_says_so_and_for_how_long() {
    let fixtures = Fixtures::new("bound");
    let file = fixtures.write("bound.dsp", BOUND);
    let (ok, stdout, stderr) = probe(&descend(&file, "x,y", "0.1", "10"));
    assert!(ok, "{stderr}");

    let (mut x, mut y, mut on_upper) = (1.0_f64, 0.0_f64, 0);
    for _ in 0..10 {
        x = (x - 0.1 * 2.0 * (x - 3.0)).clamp(0.0, 2.0);
        y = (y - 0.1 * 2.0 * (y - 0.5)).clamp(-1.0, 1.0);
        on_upper += usize::from(x >= 2.0);
    }
    assert_eq!(on_upper, 7, "the replay itself");
    let trained = lines_of(&stdout, "# trained");
    assert_eq!(
        trained[0],
        "# trained /bound/x=2.0 (on its upper bound for 7 of 10 blocks, the last one included)"
    );
    // a control that never met a bound is reported as before
    assert_eq!(trained[1], format!("# trained /bound/y={y:?}"));
    // and the lesson, naming the stopped control only
    assert_eq!(
        lines_of(&stdout, "# note"),
        ["# note: a control that ends on a bound is stopped, not converged: /bound/x"]
    );
}

#[test]
fn a_control_that_left_its_bound_is_said_to_have_left_it() {
    let fixtures = Fixtures::new("leaves");
    let file = fixtures.write("leaves.dsp", LEAVES);
    // x <- (x + t) / 2: 2, 2, 2 against the target 3, then 1.5, 1.25, 1.125
    let (ok, stdout, stderr) = probe(&descend(&file, "x", "0.25", "6"));
    assert!(ok, "{stderr}");
    assert_eq!(
        lines_of(&stdout, "# trained"),
        ["# trained /leaves/x=1.125 (on its upper bound for 3 of 6 blocks, not the last one)"]
    );
    // it is not stopped: no note
    assert!(lines_of(&stdout, "# note").is_empty(), "{stdout}");
}

#[test]
fn the_bounds_are_in_the_json_document() {
    let fixtures = Fixtures::new("bound_json");
    let file = fixtures.write("bound.dsp", BOUND);
    let mut args = descend(&file, "x,y", "0.1", "10");
    args.extend(["--format", "json"]);
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    let document = json(&stdout);
    let trained = &document["train"]["trained"];
    assert_eq!(trained[0]["path"], "/bound/x");
    assert_eq!(trained[0]["blocks_on_upper"], 7);
    assert_eq!(trained[0]["blocks_on_lower"], 0);
    assert_eq!(trained[0]["ends_on"], "upper");
    assert_eq!(trained[0]["max"], 2.0);
    assert_eq!(trained[1]["ends_on"], serde_json::Value::Null);
    assert_eq!(trained[1]["blocks_on_upper"], 0);
    assert!(
        document["train"]["notes"][0]
            .as_str()
            .unwrap()
            .contains("/bound/x")
    );
    // one document and nothing else: no CSV row, no `#` line
    assert!(stdout.trim_start().starts_with('{') && stdout.trim_end().ends_with('}'));
}

// --------------------------------------------------------------- gradients

#[test]
fn train_verbose_adds_the_mean_gradient_of_each_block() {
    let fixtures = Fixtures::new("verbose");
    let file = fixtures.write("bound.dsp", BOUND);
    let mut args = descend(&file, "x,y", "0.1", "3");
    args.push("--train-verbose");
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    let rows: Vec<&str> = stdout.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(rows[0], "block,loss,x,y,grad_x,grad_y");
    // the gradient the step was computed from: at the controls the block ran
    // with, 2 (1 - 3) and 2 (0 - 0.5) for the first
    let first: Vec<&str> = rows[1].split(',').collect();
    assert_eq!(first[4..], ["-4e0", "-1e0"]);
    let (mut x, mut y) = (1.0_f64, 0.0_f64);
    for row in &rows[1..] {
        let fields: Vec<f64> = row.split(',').map(|f| f.parse().unwrap()).collect();
        assert!((fields[4] - 2.0 * (x - 3.0)).abs() < 1e-12, "{row}");
        assert!((fields[5] - 2.0 * (y - 0.5)).abs() < 1e-12, "{row}");
        x = (x - 0.1 * fields[4]).clamp(0.0, 2.0);
        y = (y - 0.1 * fields[5]).clamp(-1.0, 1.0);
        assert!((fields[2] - x).abs() < 1e-12 && (fields[3] - y).abs() < 1e-12);
    }

    // without the flag the rows are what they always were
    let (_, plain, _) = probe(&descend(&file, "x,y", "0.1", "3"));
    let plain_rows: Vec<&str> = plain.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(plain_rows[0], "block,loss,x,y");
    assert_eq!(plain_rows[1].split(',').count(), 4);
}

#[test]
fn train_verbose_without_train_is_refused() {
    let fixtures = Fixtures::new("verbose_alone");
    let file = fixtures.write("bound.dsp", BOUND);
    let (ok, _, stderr) = probe(&["--train-verbose", &file]);
    assert!(!ok);
    assert!(stderr.contains("--train-verbose"), "{stderr}");
}

// ------------------------------------------------------------ the best loss

/// Five blocks towards 0.5 (`x <- (x + 0.5) / 2`: the losses are 0.25 / 4^k),
/// then the target jumps to 1.5.
#[test]
fn the_lowest_loss_is_reported_with_its_block_and_its_controls() {
    let fixtures = Fixtures::new("jumps");
    let file = fixtures.write("jumps.dsp", JUMPS);
    let (ok, stdout, stderr) = probe(&descend(&file, "x", "0.25", "6"));
    assert!(ok, "{stderr}");
    // block 5 ran with x = 0.53125: (0.53125 - 0.5)^2 = 9.765625e-4
    let losses = lines_of(&stdout, "# loss");
    assert_eq!(losses[1], "# loss: minimum 9.765625e-4 at block 5");
    // block 6 ran with 0.515625 against 1.5: 0.96899..., 992.25 times more
    let ratio: f64 = (0.515_625_f64 - 1.5).powi(2) / 9.765_625e-4;
    assert!((ratio - 992.25).abs() < 1e-9);
    assert_eq!(
        lines_of(&stdout, "# note"),
        [
            "# note: the last loss is 992 times the minimum, which block 5 reached with /jumps/x=0.53125"
        ]
    );

    // seven times the minimum is a block's noise, not a descent that left:
    // (0.515625 - 0.6)^2 is 7.29 times 9.765625e-4, and nothing is said
    let little = fixtures.write("little.dsp", JUMPS_A_LITTLE);
    let (ok, stdout, stderr) = probe(&descend(&little, "x", "0.25", "6"));
    assert!(ok, "{stderr}");
    assert_eq!(
        lines_of(&stdout, "# loss")[1],
        "# loss: minimum 9.765625e-4 at block 5"
    );
    assert!(lines_of(&stdout, "# note").is_empty(), "{stdout}");

    // a descent that ends at its best says nothing
    let bound = fixtures.write("bound.dsp", BOUND);
    let (_, quiet, _) = probe(&descend(&bound, "x,y", "0.1", "10"));
    assert!(lines_of(&quiet, "# loss")[1].ends_with("at block 10"));
    assert!(!quiet.contains("times the minimum"));
}

// ------------------------------------------------------ --fd-check at the end

#[test]
fn fd_check_at_the_end_catches_a_gradient_that_is_wrong_where_the_descent_stops() {
    let fixtures = Fixtures::new("wrong_later");
    let file = fixtures.write("wrong.dsp", WRONG_LATER);

    // at the start the lane is right, and the descent runs
    let mut start = descend(&file, "x", "0.1", "10");
    start.push("--fd-check");
    let (ok, stdout, stderr) = probe(&start);
    assert!(ok, "{stderr}");
    assert!(lines_of(&stdout, "# fd-check end").is_empty());

    // at the end, on the bound 2: the lane says 0.5 (2 - 3) per sample, the
    // loss 2 (2 - 3): -32 against -128 over the block
    let mut both = descend(&file, "x", "0.1", "10");
    both.push("--fd-check=both");
    let (ok, stdout, stderr) = probe(&both);
    assert!(!ok, "a wrong gradient at the end must fail the command");
    assert!(stderr.contains("at the end of the descent"), "{stderr}");
    assert_eq!(lines_of(&stdout, "# fd-check /wrong/x").len(), 1);
    let end = lines_of(&stdout, "# fd-check end /wrong/x");
    assert_eq!(end.len(), 1, "{stdout}");
    assert!(end[0].contains("rad -3.200000e1"), "{}", end[0]);
    assert!(end[0].contains("relative error 7.50e-1"), "{}", end[0]);
    // the rows and the trained value come before the verdict
    assert!(stdout.contains("# trained /wrong/x=2.0"));

    // `end` alone does not check the start
    let mut end_only = descend(&file, "x", "0.1", "10");
    end_only.push("--fd-check=end");
    let (ok, stdout, _) = probe(&end_only);
    assert!(!ok);
    assert!(lines_of(&stdout, "# fd-check /wrong/x").is_empty());
    assert_eq!(lines_of(&stdout, "# fd-check end").len(), 2);
}

/// At a minimum the gradient is zero and a central difference is not: it is
/// off by `f''' h^2 / 6`, here `50^3 * 1e-6 / 6` per sample, 1.33 over a
/// block of 64, which a check against the plain difference reports as an
/// error of the gradient. The reference is extrapolated from the steps `h`
/// and `h / 2`, which leaves `-a^5 h^4 / 480` per sample: -4.2e-5.
#[test]
fn the_reference_of_fd_check_is_accurate_where_the_gradient_vanishes() {
    let fixtures = Fixtures::new("curved");
    let file = fixtures.write("curved.dsp", CURVED);
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "--in",
        "zero",
        "--block",
        "64",
        "--train",
        "x",
        "--blocks",
        "0",
        "--fd-check",
        "--format",
        "json",
        &file,
    ]);
    assert!(ok, "a right gradient must pass: {stderr}");
    let check = &json(&stdout)["train"]["fd_check"]["start"]["checks"][0];
    assert_eq!(check["rad"], 0.0);
    let (a, h) = (50.0_f64, 1e-3_f64);
    let plain = 64.0 * ((a * h).sinh() - a * h) / h;
    assert!((plain - 1.3335).abs() < 1e-4, "the closed form itself");
    assert!((check["fd_plain"].as_f64().unwrap() - plain).abs() < 1e-9);
    let reference = check["fd"].as_f64().unwrap();
    assert!(
        (reference + 64.0 * a.powi(5) * h.powi(4) / 480.0).abs() < 1e-6,
        "{reference}"
    );
    assert!(check["relative_error"].as_f64().unwrap() < 1e-4);
}

#[test]
fn fd_check_at_the_end_needs_a_descent_and_the_bare_flag_still_takes_no_value() {
    let fixtures = Fixtures::new("fd_args");
    let file = fixtures.write("bound.dsp", BOUND);
    let (ok, _, stderr) = probe(&[
        "--double",
        "--train",
        "x,y",
        "--blocks",
        "0",
        "--fd-check=end",
        &file,
    ]);
    assert!(!ok);
    assert!(stderr.contains("needs a descent"), "{stderr}");
    // `--fd-check FILE`: the file is the program, not the flag's value
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "--in",
        "zero",
        "--train",
        "x,y",
        "--blocks",
        "0",
        "--fd-check",
        &file,
    ]);
    assert!(ok, "{stderr}");
    assert_eq!(lines_of(&stdout, "# fd-check /bound/").len(), 2);
}

// ------------------------------------------------------- grid, then descent

fn wells(x: f64) -> f64 {
    (x * x - 1.0).powi(2) + 0.3 * x
}

#[test]
fn a_grid_of_starting_points_then_the_descent_from_the_best() {
    let fixtures = Fixtures::new("wells");
    let file = fixtures.write("wells.dsp", WELLS);

    // from the slider's 1 the descent stops in the local minimum
    let (ok, stdout, stderr) = probe(&descend(&file, "x", "0.05", "200"));
    assert!(ok, "{stderr}");
    let local: f64 = lines_of(&stdout, "# trained")[0]
        .rsplit('=')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!((local - 0.9601).abs() < 1e-3, "{local}");

    // the same command with a grid finds the other well
    let mut args = descend(&file, "x", "0.05", "200");
    args.extend(["--sweep", "x=-1.5,-0.5,0.5,1.5", "--format", "json"]);
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    let document = json(&stdout);
    let grid = &document["train"]["grid"];
    let points = grid["points"].as_array().unwrap();
    assert_eq!(points.len(), 4);
    for (point, x) in points.iter().zip([-1.5, -0.5, 0.5, 1.5]) {
        assert_eq!(point["set"]["/wells/x"], x);
        let loss = point["loss"].as_f64().unwrap();
        assert!((loss - wells(x)).abs() < 1e-12, "{x}: {loss}");
    }
    // -0.5 is the lowest of the four, and the descent's first block is the
    // grid's block there: the very same number
    assert_eq!(grid["best"], 1);
    assert_eq!(document["train"]["loss"]["first"], points[1]["loss"]);
    let global = document["train"]["trained"][0]["value"].as_f64().unwrap();
    assert!((global + 1.0356).abs() < 1e-3, "{global}");
    assert!(wells(global) < wells(local) - 0.5);
}

#[test]
fn the_grid_lines_name_the_best_point_and_precede_the_descent() {
    let fixtures = Fixtures::new("grid_lines");
    let file = fixtures.write("bound.dsp", BOUND);
    // two axes, the last varying fastest; the losses are exact:
    // (x - 3)^2 + (y - 0.5)^2
    let mut args = descend(&file, "x,y", "0.1", "1");
    args.extend(["--sweep", "x=0,2", "--sweep", "y=-1,1"]);
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    assert_eq!(
        lines_of(&stdout, "# grid /"),
        [
            "# grid /bound/x=0 /bound/y=-1 loss=1.125e1",
            "# grid /bound/x=0 /bound/y=1 loss=9.25e0",
            "# grid /bound/x=2 /bound/y=-1 loss=3.25e0",
            "# grid /bound/x=2 /bound/y=1 loss=1.25e0 (best)",
        ]
    );
    let grid_at = stdout.find("# grid:").expect("the grid's summary");
    let rows_at = stdout.find("block,loss").expect("the descent's header");
    assert!(grid_at < rows_at);
    // the descent left from (2, 1): its first block has the grid's loss
    assert!(stdout.contains("# loss: block 1 1.250000e0"), "{stdout}");
}

#[test]
fn a_fixed_control_is_written_at_every_grid_point_and_a_tie_keeps_the_first() {
    let fixtures = Fixtures::new("offset");
    let file = fixtures.write("offset.dsp", OFFSET);
    // (x - 1.5)^2 at -1, 0, 1, 2: 6.25, 2.25, 0.25, 0.25
    let mut args = descend(&file, "x", "0.1", "1");
    args.extend(["--set", "c=1.5", "--sweep", "x=-1,0,1,2"]);
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    let grid = lines_of(&stdout, "# grid /");
    assert_eq!(grid[0], "# grid /offset/x=-1 loss=6.25e0");
    assert_eq!(grid[2], "# grid /offset/x=1 loss=2.5e-1 (best)");
    assert_eq!(grid[3], "# grid /offset/x=2 loss=2.5e-1");
}

#[test]
fn a_swept_control_must_be_trained_and_its_values_in_range() {
    let fixtures = Fixtures::new("grid_errors");
    let file = fixtures.write("offset.dsp", OFFSET);
    let mut not_trained = descend(&file, "x", "0.1", "1");
    not_trained.extend(["--sweep", "c=0,1"]);
    let (ok, _, stderr) = probe(&not_trained);
    assert!(!ok);
    assert!(
        stderr.contains("must be a trained one") && stderr.contains("/offset/c"),
        "{stderr}"
    );

    let mut outside = descend(&file, "x", "0.1", "1");
    outside.extend(["--sweep", "x=0,7"]);
    let (ok, _, stderr) = probe(&outside);
    assert!(!ok);
    assert!(stderr.contains("outside the range"), "{stderr}");

    // under --clamp the grid uses the clamped value and says so
    outside.push("--clamp");
    let (ok, stdout, stderr) = probe(&outside);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("# clamped /offset/x: 7 -> 2"), "{stdout}");
    assert!(stdout.contains("# grid /offset/x=2 loss=4e0"), "{stdout}");
}

// ------------------------------------------------------------ the document

#[test]
fn the_json_document_of_a_descent() {
    let fixtures = Fixtures::new("document");
    let file = fixtures.write("bound.dsp", BOUND);
    let mut args = descend(&file, "x,y", "0.1", "10");
    args.extend(["--format", "json", "--fd-check=both"]);
    // thinned like the rows: `--every 4` keeps 4, 8 and the last
    let every = args.iter().position(|a| *a == "--every").unwrap();
    args[every + 1] = "4";
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    let document = json(&stdout);
    assert_eq!(document["schema_version"], 1);
    let train = &document["train"];
    assert_eq!(train["options"]["optimizer"], "sgd");
    assert_eq!(train["options"]["blocks"], 10);
    let rows = train["rows"].as_array().unwrap();
    let blocks: Vec<u64> = rows.iter().map(|r| r["block"].as_u64().unwrap()).collect();
    assert_eq!(blocks, [4, 8, 10]);
    // the gradients are always there: a reader ignores what it does not use
    assert_eq!(rows[0]["grads"].as_array().unwrap().len(), 2);
    assert_eq!(train["loss"]["min_block"], 10);
    assert_eq!(train["loss"]["values_at_min"].as_array().unwrap().len(), 2);
    assert_eq!(train["fd_check"]["start"]["passes"], true);
    assert_eq!(train["fd_check"]["end"]["passes"], true);
    assert_eq!(
        train["fd_check"]["end"]["checks"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn a_failed_fd_check_still_prints_the_document() {
    let fixtures = Fixtures::new("document_fails");
    let file = fixtures.write("wrong.dsp", WRONG_LATER);
    let mut args = descend(&file, "x", "0.1", "10");
    args.extend(["--format", "json", "--fd-check=end"]);
    let (ok, stdout, stderr) = probe(&args);
    assert!(!ok);
    assert!(stderr.contains("at the end of the descent"), "{stderr}");
    let document = json(&stdout);
    assert_eq!(document["train"]["fd_check"]["end"]["passes"], false);
    assert_eq!(document["train"]["rows"].as_array().unwrap().len(), 10);
}

// ------------------------------------------------------------------ failures

#[test]
fn a_loss_that_is_not_finite_names_the_block_and_its_controls() {
    let fixtures = Fixtures::new("pole");
    let file = fixtures.write("pole.dsp", POLE);
    let (ok, _, stderr) = probe(&descend(&file, "x", "0.1", "3"));
    assert!(!ok);
    assert!(
        stderr.contains("the loss is not finite at block 1"),
        "{stderr}"
    );
    assert!(
        stderr.contains("controls of that block: /pole/x=1"),
        "{stderr}"
    );
}

#[test]
fn format_ir_is_refused_with_train() {
    let fixtures = Fixtures::new("ir");
    let file = fixtures.write("bound.dsp", BOUND);
    let mut args = descend(&file, "x,y", "0.1", "1");
    args.extend(["--format", "ir"]);
    let (ok, _, stderr) = probe(&args);
    assert!(!ok);
    assert!(stderr.contains("--format ir"), "{stderr}");
}

/// The bound `0.7` reaches the host as the `f32` nearest to it, 0.699999988:
/// a starting point typed `0.7` is in range and is `0.7`, not the bound.
#[test]
fn a_starting_point_typed_on_a_decimal_bound_is_what_was_typed() {
    let fixtures = Fixtures::new("decimal");
    let file = fixtures.write("identity.dsp", IDENTITY);
    let mut args = descend(&file, "x", "0.1", "1");
    args.extend(["--set", "x=0.7", "--format", "json"]);
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    // the loss lane is the control itself: 0.7 to the rounding of a mean of
    // 64 samples, where the bound is 1.2e-8 away
    let first = json(&stdout)["train"]["loss"]["first"].as_f64().unwrap();
    assert!((first - 0.7).abs() < 1e-12, "{first}");
}

/// The `--set` values are parsed before any is checked, as in every other
/// mode: of a malformed one and one out of range, the malformed one is
/// reported. `--train` used to check them one at a time.
#[test]
fn a_malformed_set_is_reported_before_one_out_of_range() {
    let fixtures = Fixtures::new("set_order");
    let file = fixtures.write("bound.dsp", BOUND);
    let (ok, _, stderr) = probe(&[
        "--double", "--in", "zero", "--train", "x", "--set", "x=-50", "--set", "bad", "--blocks",
        "5", &file,
    ]);
    assert!(!ok);
    assert!(
        stderr.contains("expected PATH=VALUE, got `bad`"),
        "{stderr}"
    );
    assert!(!stderr.contains("outside the range"), "{stderr}");
}

/// A descent starts from the value the clamp reported: a NaN is the control's
/// initial value, not a NaN the first block turns into a loss that is not
/// finite.
#[test]
fn a_descent_starts_from_the_value_the_clamp_reported() {
    let fixtures = Fixtures::new("nan_start");
    let file = fixtures.write("bound.dsp", BOUND);
    let (ok, stdout, stderr) = probe(&[
        "--double", "--in", "zero", "--train", "x", "--set", "x=nan", "--clamp", "--lr", "0.01",
        "--blocks", "1", &file,
    ]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("# clamped /bound/x: NaN -> 1\n"),
        "{stdout}"
    );
    assert!(!stdout.contains("NaN\n"), "{stdout}");
    // the loss of the first block is that of x = 1: (1 - 3)^2 + (0 - 0.5)^2
    assert!(stdout.contains("# loss: block 1 4.250000e0"), "{stdout}");
}

/// The runtime kept a slider's initial value as `f32` and widened it into the
/// `f64` zone of a `-double` program: `0.02` started at
/// 0.019999999552965164 where `--set lr=0.02` delivered 0.02, and a descent
/// stepping by that value followed another trajectory than the same descent
/// with the slider set. The default is kept at the FIR's precision.
#[test]
fn a_slider_starts_from_the_double_its_source_wrote() {
    let source = "process = hslider(\"lr\", 0.02, 0.0, 1.0, 0.0001);\n";
    let (ok, by_default, stderr) = probe_source(
        "slider_default_double.dsp",
        source,
        &["--double", "--in", "zero", "-n", "1"],
    );
    assert!(ok, "{stderr}");
    let (ok, by_set, stderr) = probe_source(
        "slider_set_double.dsp",
        source,
        &["--double", "--in", "zero", "-n", "1", "--set", "lr=0.02"],
    );
    assert!(ok, "{stderr}");
    let row = |out: &str| {
        out.lines()
            .find(|l| l.starts_with("0,"))
            .map(str::to_owned)
            .unwrap_or_default()
    };
    assert_eq!(
        row(&by_default),
        "0,0.02",
        "the default must be the very double of the source:\n{by_default}"
    );
    assert_eq!(
        row(&by_set),
        row(&by_default),
        "a --set of the default must be the same bits"
    );
}
