//! Signal type system for the Faust compiler.
//!
//! # Source provenance (C++)
//! - `compiler/signals/sigtype.hh` — type hierarchy, enums, factories, casts
//! - `compiler/signals/sigtype.cpp` — constructors, equality, operators
//! - `compiler/signals/sigtyperules.cpp` — type inference and fixed-point loop
//!
//! # API surface
//! - [`SigType`] — unified signal type (Simple / Table / Tuplet)
//! - [`Nature`], [`Variability`], [`Computability`], [`Vectorability`],
//!   [`Boolean`], [`Res`] — lattice enums and resolution struct
//! - [`factory`] — `make_simple`, `make_table_type`, `make_tuplet`
//! - [`ops`] — `union_types`, `product_types`, cast helpers, merge, check*
//! - [`rules::TypeAnnotator`] — bottom-up signal type inference

pub mod api;
pub mod enums;
pub mod factory;
pub mod ops;
pub mod rules;
pub mod types;

pub use enums::{Boolean, Computability, Nature, Res, Variability, Vectorability};
pub use factory::{
    make_maximal, make_simple, make_simple_with_res, make_table_type, make_table_type_with,
    make_tuplet,
};
pub use ops::{
    TypeError, bit_cast, bool_cast, cast_interval, check_delay_interval, check_init, check_int,
    check_int_param, check_konst, float_cast, int_cast, merge_boolean, merge_computability,
    merge_interval, merge_nature, merge_variability, merge_vectorability, num_cast, product_types,
    samp_cast, union_types,
};
pub use rules::{InferenceError, TypeAnnotator};
pub use types::{SigType, SimpleType, TableType, TupletType};
