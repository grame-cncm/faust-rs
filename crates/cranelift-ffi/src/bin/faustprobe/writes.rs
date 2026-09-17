//! The writes of a run (`--set`, `--at`, the values of a `--sweep`), checked
//! against their controls' ranges before any render.
//!
//! A value outside its control's range is clamped by the render: a run at the
//! clamped value labelled with the requested one looks like a measurement of
//! the requested one. It is an error, or under `--clamp` a clamp that is
//! reported, and everything a run will write is checked before it renders.

use cranelift_ffi::probe::engine::{PolyProbe, PolyWrite, Probe};
use cranelift_ffi::probe::params::ControlMap;
use cranelift_ffi::probe::sweep::{Axis, Point};

use crate::report::json_number;

/// A value that was clamped under `--clamp`, for the notices.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Clamped {
    pub(crate) path: String,
    pub(crate) requested: f64,
    pub(crate) applied: f64,
}

impl Clamped {
    pub(crate) fn line(&self) -> String {
        format!(
            "# clamped {}: {} -> {}",
            self.path, self.requested, self.applied
        )
    }

    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "path": self.path,
            "requested": json_number(self.requested),
            "applied": json_number(self.applied),
        })
    }
}

/// Validates one write before any render and returns the value the control
/// takes. Outside the range: an error, or under `--clamp` a recorded clamp.
pub(crate) fn check_value(
    controls: &ControlMap,
    query: &str,
    value: f64,
    clamp: bool,
    clamped: &mut Vec<Clamped>,
) -> Result<f64, String> {
    let write = controls.check_write(query, value)?;
    if !write.in_range() {
        if !clamp {
            return Err(format!(
                "{} (--clamp accepts it, clamped to the range)",
                write.range_error(query)
            ));
        }
        clamped.push(Clamped {
            path: write.control.path.clone(),
            requested: value,
            applied: write.applied,
        });
    }
    Ok(write.applied)
}

/// [`check_value`] for a write broadcast to a polyphonic instrument: the
/// control of every voice, the effect's, or both, each against its own range.
pub(crate) fn check_poly_value(
    poly: &PolyProbe,
    query: &str,
    value: f64,
    clamp: bool,
    clamped: &mut Vec<Clamped>,
) -> Result<(), String> {
    for PolyWrite { target, write } in poly.check_write(query, value)? {
        if write.in_range() {
            continue;
        }
        if !clamp {
            return Err(format!(
                "{} {} (--clamp accepts it, clamped to the range)",
                write.range_error(query),
                target.place()
            ));
        }
        let record = Clamped {
            path: write.control.path.clone(),
            requested: value,
            applied: write.applied,
        };
        // a control written by `--set` and again by an `--at` is one clamp
        if !clamped.contains(&record) {
            clamped.push(record);
        }
    }
    Ok(())
}

/// The clamps of a run that may sweep: those every render runs under, and
/// those of the sweep's values, each of which concerns the points that use it.
#[derive(Debug, Default)]
pub(crate) struct Clamps {
    /// Of `--set`, of `--at`, and under `--compare` of the other program.
    pub(crate) fixed: Vec<Clamped>,
    /// Of a sweep's values, with the index of their axis.
    axes: Vec<(usize, Clamped)>,
}

impl Clamps {
    /// Checks every value of every axis of a sweep.
    pub(crate) fn check_axes(
        &mut self,
        controls: &ControlMap,
        axes: &[Axis],
        clamp: bool,
    ) -> Result<(), String> {
        for (index, axis) in axes.iter().enumerate() {
            let mut of_axis = Vec::new();
            for value in &axis.values {
                check_value(controls, &axis.path, *value, clamp, &mut of_axis)?;
            }
            self.axes.extend(of_axis.into_iter().map(|c| (index, c)));
        }
        Ok(())
    }

    /// All of them, the fixed ones first: what is said once, before the rows
    /// of a sweep.
    pub(crate) fn all(&self) -> impl Iterator<Item = &Clamped> {
        self.fixed.iter().chain(self.axes.iter().map(|(_, c)| c))
    }

    /// Those one point ran under: the fixed ones, and those of its own sweep
    /// values (the axes and a point's assignments are in the same order).
    pub(crate) fn of_point(&self, point: &Point) -> Vec<&Clamped> {
        self.fixed
            .iter()
            .chain(self.axes.iter().filter_map(|(axis, c)| {
                (point.assignments.get(*axis).map(|(_, v)| v.to_bits())
                    == Some(c.requested.to_bits()))
                .then_some(c)
            }))
            .collect()
    }
}

/// The values a point's renders use, as (query, applied): the requested ones,
/// unless `--clamp` clamped them, and then a row must not claim the requested
/// one.
pub(crate) fn applied(controls: &ControlMap, point: &Point) -> Vec<(String, f64)> {
    point
        .assignments
        .iter()
        .map(|(query, value)| {
            let applied = controls
                .check_write(query, *value)
                .map_or(*value, |write| write.applied);
            (query.clone(), applied)
        })
        .collect()
}

/// The controls a render wrote before its first frame, as (path, applied
/// value): what a failure is explained with.
pub(crate) fn written(
    controls: &ControlMap,
    fixed: &[(&str, f64)],
    point: &Point,
) -> Vec<(String, f64)> {
    fixed
        .iter()
        .map(|(path, value)| (*path, *value))
        .chain(point.assignments.iter().map(|(p, v)| (p.as_str(), *v)))
        .filter_map(|(query, value)| controls.check_write(query, value).ok())
        .map(|write| (write.control.path.clone(), write.applied))
        .collect()
}

/// Every point starts from the same known state (see `probe::sweep`): a
/// cleared instance, the `--set` values, then the point's.
pub(crate) fn prime(probe: &Probe, fixed: &[(&str, f64)], point: &Point) -> Result<(), String> {
    probe.reset();
    for (path, value) in fixed {
        probe.set(path, *value)?;
    }
    for (path, value) in &point.assignments {
        probe.set(path, *value)?;
    }
    Ok(())
}
