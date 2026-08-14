//! Scheduled-event timing on the scalar probe (design phase P5).
//!
//! The unit tests in `probe::schedule` cover parsing and ordering. This covers
//! the property that only exists once a render runs: an event lands on the
//! frame it was asked for, not on the next block boundary.
//!
//! That property is easy to lose and hard to notice — a scheme that rounded
//! events to the block grid would still produce a plausible render, just with
//! a timing error up to one block wide baked into every measurement taken
//! from it.

use cranelift_ffi::probe::engine::{Factory, Probe, RenderSpec};
use cranelift_ffi::probe::render::InputMode;
use cranelift_ffi::probe::schedule::{Event, Schedule};
use std::rc::Rc;

/// A gain the schedule can step, fed with DC so the output *is* the gain.
const GAIN_DSP: &str = r#"
process = _ * hslider("g", 1, 0, 1, 0.001);
"#;

fn probe() -> Probe {
    let factory = Factory::compile_from_string("sched_test", GAIN_DSP, &[], false, 0)
        .expect("compile gain dsp");
    Probe::instantiate(&Rc::new(factory), 48_000).expect("instantiate")
}

/// Render with `schedule` and return every output sample.
fn render_with(schedule: Schedule, frames: usize, block: usize) -> Vec<f64> {
    let probe = probe();
    let spec = RenderSpec {
        frames,
        block,
        input: InputMode::Dc,
        skip: 0,
        schedule,
        drive_buttons: false,
    };
    let mut out = Vec::with_capacity(frames);
    probe.render(&spec, |_frame, samples| out.push(samples[0]));
    out
}

#[test]
fn a_scheduled_change_lands_on_its_exact_frame() {
    let mut schedule = Schedule::new();
    schedule.push(
        1_000,
        Event::SetParam {
            path: "g".to_owned(),
            value: 0.25,
        },
    );
    // Block 64 does not divide 1000, so a block-aligned scheme would fire at
    // 1024 and this would fail by 24 frames.
    let out = render_with(schedule, 1_200, 64);
    assert!((out[999] - 1.0).abs() < 1e-6, "changed early: {}", out[999]);
    assert!((out[1_000] - 0.25).abs() < 1e-6, "late: {}", out[1_000]);
}

#[test]
fn event_timing_is_independent_of_block_size() {
    // The same schedule must produce the same signal at any block size; if it
    // does not, the block is leaking into the result.
    let build = || {
        let mut s = Schedule::new();
        s.push(
            777,
            Event::SetParam {
                path: "g".to_owned(),
                value: 0.5,
            },
        );
        s
    };
    let a = render_with(build(), 1_024, 64);
    let b = render_with(build(), 1_024, 256);
    let c = render_with(build(), 1_024, 1);
    assert_eq!(a, b);
    assert_eq!(a, c);
}

#[test]
fn several_events_apply_in_order_at_the_same_frame() {
    let mut schedule = Schedule::new();
    for value in [0.1, 0.2, 0.75] {
        schedule.push(
            10,
            Event::SetParam {
                path: "g".to_owned(),
                value,
            },
        );
    }
    let out = render_with(schedule, 32, 8);
    // Last write wins, and it is visible from frame 10.
    assert!((out[9] - 1.0).abs() < 1e-6);
    assert!((out[10] - 0.75).abs() < 1e-6);
}

#[test]
fn an_event_at_frame_zero_applies_before_the_first_sample() {
    let mut schedule = Schedule::new();
    schedule.push(
        0,
        Event::SetParam {
            path: "g".to_owned(),
            value: 0.0,
        },
    );
    let out = render_with(schedule, 16, 8);
    assert!(out.iter().all(|v| v.abs() < 1e-9), "first sample leaked");
}

#[test]
fn an_event_past_the_render_is_simply_never_applied() {
    let mut schedule = Schedule::new();
    schedule.push(
        10_000,
        Event::SetParam {
            path: "g".to_owned(),
            value: 0.0,
        },
    );
    let out = render_with(schedule, 64, 64);
    assert!(out.iter().all(|v| (v - 1.0).abs() < 1e-6));
}
