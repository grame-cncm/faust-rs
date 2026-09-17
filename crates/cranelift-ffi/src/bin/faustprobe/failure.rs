//! A render that went wrong: where it starts, and what it is explained with.
//!
//! A runaway is reported where it starts: a loop that leaves its stable region
//! passes any level long before it overflows, so the sample above
//! `--fail-above` comes first, and the non-finite frame that follows is
//! mentioned with it. Either way the error carries the controls written by
//! that frame and the last scheduled write before it, and for an instrument
//! the notes held then.

use cranelift_ffi::probe::engine::{PolyProbe, PolyWrite};
use cranelift_ffi::probe::number::NumberFormat;
use cranelift_ffi::probe::params::ControlMap;
use cranelift_ffi::probe::render::{Located, RenderStats};
use cranelift_ffi::probe::schedule::Schedule;

use crate::report::non_finite_name;

/// The failure of a render, if it has one.
///
/// `non_finite` is the first sample that is not finite when that fails the
/// render: the `.ir` protocol passes `None`, its reference corpus holding
/// programs whose expected output has NaN in it. `context` explains a frame.
pub(crate) fn render_failure(
    stats: &RenderStats,
    non_finite: Option<(usize, Located)>,
    fail_above: Option<f64>,
    frames: usize,
    fmt: &NumberFormat,
    context: impl Fn(usize) -> String,
) -> Option<String> {
    if let Some((channel, located)) = stats.first_above()
        && non_finite.is_none_or(|(_, nf)| located.frame <= nf.frame)
    {
        let later = non_finite.map_or_else(String::new, |(ch, nf)| {
            format!(
                "\n  the render turns non-finite at frame {}, out{ch} ({})",
                nf.frame,
                non_finite_name(nf.value)
            )
        });
        return Some(format!(
            "a sample exceeds --fail-above {}\n  first: frame {}, out{channel} = {}{later}{}",
            fail_above.unwrap_or_default(),
            located.frame,
            fmt.sample(located.value),
            context(located.frame)
        ));
    }
    let (channel, located) = non_finite?;
    Some(format!(
        "render produced non-finite samples\n  first: frame {}, out{channel} ({}); {} of {} frames affected{}",
        located.frame,
        non_finite_name(located.value),
        stats.non_finite_frames,
        frames,
        context(located.frame)
    ))
}

/// The value of each written control at a frame, and the last scheduled write
/// before it.
#[derive(Debug, Default)]
struct WrittenBy {
    then: Vec<(String, f64)>,
    last_event: Option<(usize, String, f64)>,
}

impl WrittenBy {
    /// A write before the first frame: `--set`, a sweep's value.
    fn record(&mut self, path: &str, applied: f64) {
        match self.then.iter_mut().find(|(p, _)| p == path) {
            Some(entry) => entry.1 = applied,
            None => self.then.push((path.to_owned(), applied)),
        }
    }

    /// A write of `--at`, at frame `at`.
    fn scheduled(&mut self, at: usize, path: &str, applied: f64) {
        self.record(path, applied);
        self.last_event = Some((at, path.to_owned(), applied));
    }

    fn text(&self) -> String {
        let mut text = String::new();
        if self.then.is_empty() {
            text.push_str("\n  controls then: all at their initial values");
        } else {
            let listed: Vec<String> = self.then.iter().map(|(p, v)| format!("{p}={v}")).collect();
            text.push_str(&format!(
                "\n  controls written by then: {}",
                listed.join(" ")
            ));
        }
        if let Some((at, path, value)) = &self.last_event {
            text.push_str(&format!(
                "\n  last scheduled write before it: frame {at}, {path}={value}"
            ));
        }
        text
    }
}

/// What a failed render is explained with: where it failed, the controls
/// written by then, and the last scheduled write before it.
pub(crate) fn failure_context(
    frame: usize,
    written: &[(String, f64)],
    schedule: &Schedule,
    controls: &ControlMap,
) -> String {
    // the value of each written control at `frame`: its `--set` or sweep
    // value, then every scheduled write up to that frame
    let mut by = WrittenBy {
        // as they were written, a control given twice listed twice
        then: written.to_vec(),
        last_event: None,
    };
    for (at, query, value) in schedule.param_writes() {
        if at > frame {
            break;
        }
        let Ok(write) = controls.check_write(query, value) else {
            continue;
        };
        by.scheduled(at, &write.control.path, write.applied);
    }
    by.text()
}

/// [`failure_context`] for a polyphonic render: the controls written by the
/// failing frame, on the voices and on the effect, the last scheduled write
/// before it, and the notes held then, which for an instrument is half of
/// what a failure is explained with.
pub(crate) fn poly_failure_context(
    frame: usize,
    fixed: &[(&str, f64)],
    schedule: &Schedule,
    poly: &PolyProbe,
) -> String {
    // where a broadcast write lands: every voice's control, the effect's, or both
    let landing = |query: &str, value: f64| -> Vec<(String, f64)> {
        poly.check_write(query, value)
            .unwrap_or_default()
            .into_iter()
            .map(|PolyWrite { write, .. }| (write.control.path.clone(), write.applied))
            .collect()
    };
    let mut by = WrittenBy::default();
    for (query, value) in fixed {
        for (path, applied) in landing(query, *value) {
            by.record(&path, applied);
        }
    }
    for (at, query, value) in schedule.param_writes() {
        if at > frame {
            break;
        }
        for (path, applied) in landing(query, value) {
            by.scheduled(at, &path, applied);
        }
    }
    let mut text = by.text();
    let held = schedule.notes_held_at(frame);
    if held.is_empty() {
        text.push_str("\n  notes held then: none");
    } else {
        let listed: Vec<String> = held
            .iter()
            .map(|(pitch, on)| format!("{pitch} (on at frame {on})"))
            .collect();
        text.push_str(&format!("\n  notes held then: {}", listed.join(", ")));
    }
    text
}
