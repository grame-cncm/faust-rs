//! Checked vector (`-vec`) pipeline: authoritative artifact-flow map.
//!
//! Every stage produces a versioned artifact wrapped in an opaque
//! `Verified*` type that only its producer/checker boundary can construct.
//! **A checker never calls its producer, never reuses a producer cache, and
//! never accepts a producer-derived expected result as evidence** — it
//! re-derives the facts it validates. This intentional duplication is the
//! assurance boundary; do not "deduplicate" it.
//!
//! # Stage map
//!
//! | Stage (module) | Input | Output artifact | Independent check |
//! |---|---|---|---|
//! | [`analysis`] | `VerifiedPreparedSignals` | execution conditions (DNF), dependency/occurrence views, effect summaries, use tables | source-aligned decoration recomputation ([`super::decoration_verify`]) |
//! | [`plan`] | analysis decorations | `VectorPlan`: signal→loop placement, transports, epochs, fused serial groups | [`verify`] re-validates coverage, order, effects, witnesses with its own reachability |
//! | [`schedule`] | verified plan | strategy-dependent epoch serialization (`-ss 0..3`) | deterministic-order audit; plan certificate is strategy-independent |
//! | [`state`] | plan + decorations | `VectorStatePlan`: delay/recursion `pre/exec/post` phases, C++ copy/ring words | independent geometry/phase-coverage validation + executable simulators |
//! | [`clock_ad`] | plan + fresh clock envs | `VectorClockAdPlan`: OD/US/DS islands, AD policy, fire-time cursors | independent source/domain alignment check; shared admission guards stay on both paths (plan §4.8) |
//! | [`route`] | plan + state/clock policies | `VerifiedRoutedFir`: region-local definitions, transport materialization | independent FIR-evidence inspection |
//! | [`lower`] | routed regions | `VerifiedPureVectorProgram`: lowered signal closures, per-region CSE | final bodies re-checked against route evidence |
//! | [`events`] | routed chunk | `VerifiedEventOrderCertificate`: scalar/vector dynamic orders, `FissionSafe` obligation | independent event/dependency reconstruction |
//! | [`assemble`] | lowered program + state/clock plans | `VerifiedVectorFirAssembly`: loops, state words, islands | independent exact-coverage FIR inspection |
//! | `module` | assembly | final FIR module: outputs, lifecycle, `-lv 0/1` chunk drivers | FIR checker + vector-specific final checks; maps rejections to fallback reasons |
//! | [`lockstep`] | prepared groups | instance-vectorization isomorphism candidates | witness-based independent gate |
//! | `ui` | `UiProgram` | `VectorUiFir`: zone declarations, `instanceResetUserInterface` and grouped `buildUserInterface` statements (builder shared with the scalar lowerer) | final-module check of exact declaration/reset/UI-statement coverage and UI-write attribution |
//!
//! Semantic contracts frozen across refactors: deterministic ordering of
//! every schedule and serialized certificate; exact schema versions and
//! stable diagnostic codes; fail-closed fallback reasons; scalar/vector
//! bit-exactness; `ZeroPad` fire-index gating; history-served cross-group
//! back-edges; `attach` ordering edges; `UnadoptedStatefulRead` rejection.
//!
//! # C++ provenance
//! The pipeline is an adapted decomposition of the C++
//! `DAGInstructionsCompiler` (`compiler/generator/dag_instructions_compiler.cpp`,
//! `compile_vect.cpp`): C++ discovers facts while lowering; this port freezes
//! each fact family into a checked artifact first, then lowers. Per-stage
//! provenance lives in each module header.
//!
//! # Plan-codename glossary
//! Stage headers cite phases of the porting plans; the leaf docs are
//! readable without opening them given this key:
//!
//! - **P4.x** — signal-level analysis and the strategy-independent plan:
//!   P4.3a effect identities, P4.3b checked decorations ([`analysis`],
//!   [`super::decoration_verify`]), P4.4 the vector plan ([`plan`],
//!   [`verify`]).
//! - **P5.x** — region-aware FIR routing and lowering: P5.1 routing
//!   evidence ([`route`]), P5.2 signal-closure lowering ([`lower`]).
//! - **P6.x** — state and clock composition: P6.1 delay/recursion state
//!   plans ([`state`]), P6.2 clock-island/AD policy ([`clock_ad`]),
//!   P6.5/P6.6 their consumption by lowering and state.
//!   (P4–P6 are phases of
//!   `porting/vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`.)
//! - **R0–R9** — the 2026-07 cleanup/documentation refactor
//!   (`porting/transform-cleanup-documentation-factorization-plan-2026-07-19-en.md`):
//!   R5–R7 split each stage into `model`/`build`(`produce`,`materialize`)/
//!   `check`/`tests` (`lower` uses `program`/`signal`/`tables`/`check`;
//!   `schedule` and `lockstep` stay single-file), R9 added the docs/layout
//!   quality gates.
//! - **§3.2 / §4.6 / §4.8** — sections of that cleanup plan: §3.2
//!   producer/checker code must not be merged; §4.6 producer and checker
//!   reachability stay disjoint; §4.8 the shared terminal verify keeps every
//!   admission guard on both the producer and the standalone checker path.
//!   Other `§` citations belong to the plan named or implied by their context
//!   (e.g. the vector-mode plan for §5.x,
//!   `porting/lean-rust-certified-porting-plan-2026-07-11-en.md` for the §8
//!   negative-testing policy).
//! - **`-ss` / `-vec` / `-vs` / `-lv`** — user-facing scheduling-strategy,
//!   vector-mode, vector-size, and chunk-driver options (Faust C++
//!   spellings).

use super::{cse, decoration_verify, recursion, siggen};

pub mod analysis;
pub mod assemble;
pub mod clock_ad;
pub(crate) mod common;
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
