//! Parameter sweeps and their reductions.
//!
//! # Why sweeps belong in the tool rather than in a shell loop
//! A shell loop re-runs the binary, which re-JITs the DSP. Cranelift renders
//! about five times faster than the interpreter but pays a fixed compile cost
//! first, so amortising that cost across configurations is the whole reason
//! the probe is built on Cranelift at all. A 30-point sweep is one JIT and 30
//! renders.
//!
//! # State between points
//! Each point renders from a clean slate: the instance's user interface is
//! reset to declared defaults, its state is cleared, then the fixed `--set`
//! values and the point's own values are applied. Without the clear, a
//! resonant filter would carry its ringing into the next point and every
//! measurement after the first would be contaminated by the one before —
//! silently, since the output would still look plausible.

use std::fmt;

/// One swept control and the values it takes.
#[derive(Debug, Clone)]
pub struct Axis {
    /// Control query, as written on the command line.
    pub path: String,
    /// Values to visit, in order.
    pub values: Vec<f64>,
}

/// What to reduce a rendered window to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reduction {
    /// Root mean square, per channel.
    Rms,
    /// Largest absolute value, per channel.
    Peak,
    /// Sum of squares, per channel.
    Energy,
    /// Mean, per channel — non-zero flags a DC offset.
    Dc,
    /// Frequency of the strongest non-DC bin, per channel.
    F0,
    /// Spurious-free dynamic range: dB from the fundamental down to the loudest
    /// component off its harmonic grid. Answers "how much aliasing is left".
    Sfdr,
    /// Total harmonic distortion: dB of harmonics 2, 3, … relative to the
    /// fundamental. The companion question to `Sfdr`, where the harmonics are
    /// what is measured rather than what is excluded.
    Thd,
}

impl fmt::Display for Reduction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rms => "rms",
            Self::Peak => "peak",
            Self::Energy => "energy",
            Self::Dc => "dc",
            Self::F0 => "f0",
            Self::Sfdr => "sfdr",
            Self::Thd => "thd",
        })
    }
}

/// One point of a sweep: the concrete values assigned to each axis.
#[derive(Debug, Clone)]
pub struct Point {
    /// `(path, value)` in axis order.
    pub assignments: Vec<(String, f64)>,
}

/// Every combination of the axes, in row-major order.
///
/// The last axis varies fastest, so a two-axis sweep reads like a table with
/// the first axis as its rows.
#[must_use]
pub fn cartesian(axes: &[Axis]) -> Vec<Point> {
    let mut points = vec![Point {
        assignments: Vec::new(),
    }];
    for axis in axes {
        let mut next = Vec::with_capacity(points.len() * axis.values.len());
        for point in &points {
            for value in &axis.values {
                let mut assignments = point.assignments.clone();
                assignments.push((axis.path.clone(), *value));
                next.push(Point { assignments });
            }
        }
        points = next;
    }
    points
}

/// Parse `PATH=V1,V2,...` into an axis.
///
/// # Errors
/// Returns a message when the assignment has no `=`, when the value list is
/// empty, or when a value is not a number.
pub fn parse_axis(text: &str) -> Result<Axis, String> {
    let (path, list) = text
        .split_once('=')
        .ok_or_else(|| format!("expected PATH=V1,V2,..., got `{text}`"))?;
    if path.is_empty() {
        return Err(format!("empty control path in `{text}`"));
    }
    let mut values = Vec::new();
    for item in list.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            return Err(format!("empty value in `{text}`"));
        }
        values.push(
            trimmed
                .parse::<f64>()
                .map_err(|_| format!("`{trimmed}` is not a number in `{text}`"))?,
        );
    }
    if values.is_empty() {
        return Err(format!("no values in `{text}`"));
    }
    Ok(Axis {
        path: path.to_owned(),
        values,
    })
}

/// Parse the text of a `--points` file: a sweep given point by point rather
/// than as a cartesian product (presets, random patches).
///
/// The first line that is neither empty nor a `#` comment names the
/// controls, separated by commas; every following one holds a point, one
/// number per control. Returns the controls as axes (each with its column's
/// values, in row order, for the range checks) and the points.
///
/// # Errors
/// A header with an empty or repeated name, a row with another number of
/// fields or a field that is not a number (with its line), no row at all.
pub fn parse_points(text: &str) -> Result<(Vec<Axis>, Vec<Point>), String> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'));
    let (_, header) = lines
        .next()
        .ok_or_else(|| "no header line naming the controls".to_owned())?;
    let mut axes: Vec<Axis> = Vec::new();
    for name in header.split(',').map(str::trim) {
        if name.is_empty() {
            return Err(format!("empty control name in the header `{header}`"));
        }
        if axes.iter().any(|axis| axis.path == name) {
            return Err(format!("`{name}` is named twice in the header"));
        }
        axes.push(Axis {
            path: name.to_owned(),
            values: Vec::new(),
        });
    }
    let mut points = Vec::new();
    for (number, line) in lines {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() != axes.len() {
            return Err(format!(
                "line {number}: {} field(s) for {} control(s)",
                fields.len(),
                axes.len()
            ));
        }
        let mut assignments = Vec::with_capacity(axes.len());
        for (axis, field) in axes.iter_mut().zip(fields) {
            let value = field
                .parse::<f64>()
                .map_err(|_| format!("line {number}: `{field}` is not a number"))?;
            axis.values.push(value);
            assignments.push((axis.path.clone(), value));
        }
        points.push(Point { assignments });
    }
    if points.is_empty() {
        return Err("no point after the header".to_owned());
    }
    Ok((axes, points))
}

/// Parse a reduction name.
///
/// # Errors
/// Returns a message listing the accepted names.
pub fn parse_reduction(name: &str) -> Result<Reduction, String> {
    match name {
        "rms" => Ok(Reduction::Rms),
        "peak" => Ok(Reduction::Peak),
        "energy" => Ok(Reduction::Energy),
        "dc" => Ok(Reduction::Dc),
        "f0" => Ok(Reduction::F0),
        "sfdr" => Ok(Reduction::Sfdr),
        "thd" => Ok(Reduction::Thd),
        other => Err(format!(
            "unknown reduction `{other}`; expected rms, peak, energy, dc, f0, sfdr or thd"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_axis_visits_every_value() {
        let axes = vec![parse_axis("cutoff=100,200,300").unwrap()];
        let points = cartesian(&axes);
        assert_eq!(points.len(), 3);
        assert!((points[2].assignments[0].1 - 300.0).abs() < f64::EPSILON);
    }

    #[test]
    fn two_axes_vary_the_last_fastest() {
        let axes = vec![
            parse_axis("a=1,2").unwrap(),
            parse_axis("b=10,20,30").unwrap(),
        ];
        let points = cartesian(&axes);
        assert_eq!(points.len(), 6);
        // Row-major: (1,10) (1,20) (1,30) (2,10) ...
        assert!((points[0].assignments[1].1 - 10.0).abs() < f64::EPSILON);
        assert!((points[1].assignments[1].1 - 20.0).abs() < f64::EPSILON);
        assert!((points[3].assignments[0].1 - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn no_axes_yields_one_empty_point() {
        // A run with no sweep is a sweep of exactly one point, so the caller
        // needs no special case.
        let points = cartesian(&[]);
        assert_eq!(points.len(), 1);
        assert!(points[0].assignments.is_empty());
    }

    #[test]
    fn rejects_malformed_axes() {
        assert!(parse_axis("cutoff").is_err());
        assert!(parse_axis("=1,2").is_err());
        assert!(parse_axis("cutoff=1,,2").is_err());
        assert!(parse_axis("cutoff=loud").is_err());
    }

    #[test]
    fn accepts_a_single_value_axis() {
        assert_eq!(parse_axis("cutoff=440").unwrap().values.len(), 1);
    }

    #[test]
    fn points_keep_their_rows() {
        let text = "# presets\ncutoff, q\n\n100, 0.5\n# a comment\n2000,4\n";
        let (axes, points) = parse_points(text).unwrap();
        assert_eq!(axes.len(), 2);
        assert_eq!(axes[1].values, [0.5, 4.0]);
        assert_eq!(points.len(), 2);
        assert_eq!(points[1].assignments[0], ("cutoff".to_owned(), 2000.0));
    }

    #[test]
    fn rejects_malformed_points() {
        for text in [
            "",
            "# only a comment",
            "a,b",
            "a,,b\n1,2,3",
            "a,a\n1,2",
            "a,b\n1",
            "a,b\n1,x",
        ] {
            assert!(parse_points(text).is_err(), "`{text}` accepted");
        }
        assert_eq!(
            parse_points("a,b\n1,2\n3").unwrap_err(),
            "line 3: 1 field(s) for 2 control(s)"
        );
    }

    #[test]
    fn rejects_unknown_reduction() {
        assert!(parse_reduction("median").is_err());
        assert_eq!(parse_reduction("rms").unwrap(), Reduction::Rms);
    }
}
