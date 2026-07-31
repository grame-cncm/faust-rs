//! Multi-rate schemas for on-demand, downsampling, and upsampling nodes.
//!
//! The three node families share [`MultiRateSchema`]; their Faust-specific
//! names are selected by the schema builder rather than represented by distinct
//! Rust types.
//!
//! All three share the same structure: an inner schema surrounded by a solid
//! border box, a rate-control input port, and a label.
//!
//! C++ references:
//! - `schema/ondemandSchema.h/cpp`
//! - `schema/downsamplingSchema.h/cpp`
//! - `schema/upsamplingSchema.h/cpp`

use crate::device::DrawDevice;
use crate::error::DrawError;
use crate::schema::{D_LETTER, Orientation, Placement, Point, Schema, Trait, TraitCollector};

const TOP_MARGIN: f64 = 30.0;
const HOR_MARGIN: f64 = 10.0;
const BOT_MARGIN: f64 = 10.0;
const MIN_WIDTH: f64 = 50.0;

/// Shared multi-rate schema body.
///
/// Wraps an inner schema with a solid-border box, a "run/rate" control input,
/// and a text label.  Inputs `= inner.inputs + 1` (clock/rate), Outputs `= inner.outputs`.
///
/// C++ reference: `ondemandSchema.cpp:50` / `downsamplingSchema.cpp:51`.
pub struct MultiRateSchema {
    width: f64,
    height: f64,
    inner: Box<dyn Schema>,
    label: &'static str,
    port_label: &'static str,
    placement: Option<Placement>,
    input_points: Vec<Point>,
    output_points: Vec<Point>,
}

impl MultiRateSchema {
    fn new(inner: Box<dyn Schema>, label: &'static str, port_label: &'static str) -> Self {
        let w = MIN_WIDTH.max(inner.width() + 2.0 * HOR_MARGIN);
        let h = inner.height() + TOP_MARGIN + BOT_MARGIN;
        let ins = inner.inputs() + 1;
        let outs = inner.outputs();
        Self {
            width: w,
            height: h,
            inner,
            label,
            port_label,
            placement: None,
            input_points: vec![Point::default(); ins],
            output_points: vec![Point::default(); outs],
        }
    }
}

impl Schema for MultiRateSchema {
    fn width(&self) -> f64 {
        self.width
    }
    fn height(&self) -> f64 {
        self.height
    }
    fn inputs(&self) -> usize {
        self.input_points.len()
    }
    fn outputs(&self) -> usize {
        self.output_points.len()
    }

    /// C++ reference: `ondemandSchema.cpp:68` — `ondemandSchema::place`.
    fn place(&mut self, ox: f64, oy: f64, orientation: Orientation) {
        self.placement = Some(Placement {
            x: ox,
            y: oy,
            orientation,
        });
        let hmargin = (self.width - self.inner.width()) / 2.0;

        match orientation {
            Orientation::LeftRight => {
                self.inner.place(ox + hmargin, oy + TOP_MARGIN, orientation);
                self.input_points[0] =
                    Point::new(ox + HOR_MARGIN / 2.0, oy + 2.0 * TOP_MARGIN / 3.0);
                for i in 1..self.input_points.len() {
                    let p = self.inner.input_point(i - 1);
                    self.input_points[i] = Point::new(ox + HOR_MARGIN / 2.0, p.y);
                }
                for i in 0..self.output_points.len() {
                    let p = self.inner.output_point(i);
                    self.output_points[i] = Point::new(ox + self.width + HOR_MARGIN / 2.0, p.y);
                }
            }
            Orientation::RightLeft => {
                self.inner.place(ox + hmargin, oy + BOT_MARGIN, orientation);
                self.input_points[0] = Point::new(
                    ox + self.width - HOR_MARGIN / 2.0,
                    oy + self.height - 2.0 * TOP_MARGIN / 3.0,
                );
                for i in 1..self.input_points.len() {
                    let p = self.inner.input_point(i - 1);
                    self.input_points[i] = Point::new(ox + self.width - HOR_MARGIN / 2.0, p.y);
                }
                for i in 0..self.output_points.len() {
                    let p = self.inner.output_point(i);
                    self.output_points[i] = Point::new(ox, p.y);
                }
            }
        }
    }

    fn placed(&self) -> bool {
        self.placement.is_some()
    }
    fn placement(&self) -> Option<&Placement> {
        self.placement.as_ref()
    }

    fn input_point(&self, i: usize) -> Point {
        self.input_points[i]
    }
    fn output_point(&self, i: usize) -> Point {
        self.output_points[i]
    }

    /// C++ reference: `ondemandSchema.cpp:132` / `downsamplingSchema.cpp:133`.
    fn draw(&self, dev: &mut dyn DrawDevice) -> Result<(), DrawError> {
        assert!(self.placed());
        self.inner.draw(dev)?;

        let p = self.placement.unwrap();
        let tw = (2 + self.label.len()) as f64 * D_LETTER;
        let x0 = p.x + HOR_MARGIN / 2.0;
        let y0 = p.y + HOR_MARGIN / 2.0;
        let x1 = p.x + self.width - HOR_MARGIN / 2.0;
        let y1 = p.y + self.height - HOR_MARGIN / 2.0;

        dev.line(x0, y0, x0, y1)?;
        dev.line(x0, y1, x1, y1)?;
        dev.line(x1, y1, x1, y0)?;
        dev.line(x0, y0, x1, y0)?;

        if p.orientation == Orientation::LeftRight {
            dev.mark_direction(x0, y0, 1)?;
        } else {
            dev.mark_direction(x1, y1, -1)?;
        }

        // clock/rate arrow
        let clock = self.input_points[0];
        dev.arrow(clock.x, clock.y, 0.0, p.orientation.sign() as i32)?;

        // schema label
        if p.orientation == Orientation::LeftRight {
            dev.label(x0 + (self.width - tw) / 2.0, y0 + 5.0, self.label)?;
        } else {
            dev.label(x0 + (self.width - tw) / 2.0, y1 - 5.0, self.label)?;
        }

        // port label
        let p0 = self.input_points[0];
        if p.orientation == Orientation::LeftRight {
            dev.label(p0.x + 2.0, p0.y, self.port_label)?;
        } else {
            dev.label(p0.x - 18.0, p0.y, self.port_label)?;
        }
        Ok(())
    }

    /// C++ reference: `ondemandSchema.cpp:191` / `downsamplingSchema.cpp:192`.
    fn collect_traits(&self, c: &mut TraitCollector) {
        assert!(self.placed());
        self.inner.collect_traits(c);

        c.add_input(self.input_points[0]);

        for i in 1..self.input_points.len() {
            let p = self.input_points[i];
            let q = self.inner.input_point(i - 1);
            c.add_input(q);
            c.add_trait(Trait::new(p, q));
        }
        for i in 0..self.output_points.len() {
            let q = self.inner.output_point(i);
            let p = self.output_points[i];
            c.add_output(q);
            c.add_trait(Trait::new(q, p));
        }
    }
}

// ─── Public constructors ───────────────────────────────────────────────────────

/// C++ reference: `ondemandSchema.cpp:41` — `schema* makeOndemandSchema`.
pub fn make_ondemand(inner: Box<dyn Schema>) -> Box<dyn Schema> {
    Box::new(MultiRateSchema::new(inner, "ondemand", "run"))
}

/// C++ reference: `downsamplingSchema.cpp:42` — `schema* makeDownsamplingSchema`.
pub fn make_downsampling(inner: Box<dyn Schema>) -> Box<dyn Schema> {
    Box::new(MultiRateSchema::new(inner, "downsampling", "rate"))
}

/// C++ reference: `upsamplingSchema.cpp:42` — `schema* makeUpsamplingSchema`.
pub fn make_upsampling(inner: Box<dyn Schema>) -> Box<dyn Schema> {
    Box::new(MultiRateSchema::new(inner, "upsampling", "rate"))
}
