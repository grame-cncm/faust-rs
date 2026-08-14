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
}

impl fmt::Display for Reduction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rms => "rms",
            Self::Peak => "peak",
            Self::Energy => "energy",
            Self::Dc => "dc",
            Self::F0 => "f0",
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
        other => Err(format!(
            "unknown reduction `{other}`; expected rms, peak, energy, dc or f0"
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
    fn rejects_unknown_reduction() {
        assert!(parse_reduction("median").is_err());
        assert_eq!(parse_reduction("rms").unwrap(), Reduction::Rms);
    }
}
