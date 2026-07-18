//! Vector FIR pipeline modules.

use super::{cse, decoration_verify, recursion, siggen};

pub mod analysis;
pub mod assemble;
pub mod clock_ad;
pub mod events;
pub mod lockstep;
pub mod lower;
pub(crate) mod module;
pub mod plan;
pub mod route;
pub mod schedule;
pub mod state;
pub(crate) mod ui;
pub mod verify;

// Keep the old internal names available while callers migrate to the grouped
// `signal_fir::vector::*` namespace.
use analysis as vector_analysis;
use assemble as vector_assemble;
use clock_ad as vector_clock_ad;
use events as vector_events;
use lower as vector_lower;
use plan as vector_plan;
use route as vector_route;
use schedule as vector_schedule;
use state as vector_state;
use ui as vector_ui;
use verify as vector_verify;
