//! Flat-box arity inference for propagation.
//!
//! This module computes the input/output bus widths for validated post-eval box
//! trees, including the adapted arity rules for FAD/RAD nodes. Results are
//! memoized through [`ArityCache`] so shared box DAGs are visited once.

use super::*;

/// Builds the canonical ordered list of `n` input bus signals (`sigInput(0)` … `sigInput(n-1)`).
///
/// Output order is stable and follows input bus index order: `0..n-1`.
#[must_use]
pub fn make_sig_input_list(arena: &mut TreeArena, n: usize) -> Vec<SigId> {
    let mut b = SigBuilder::new(arena);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let index = i32::try_from(i).unwrap_or(i32::MAX);
        out.push(b.input(index));
    }
    out
}

/// Infers input/output arity of one flat post-eval box expression (memoized).
///
/// This is the typed entry point for post-`eval/a2sb` callers that already
/// hold a validated [`FlatBoxId`].
///
/// AD arity note:
/// - `fad(expr, seed)` reports **expanded** output arity:
///   `outputs = body_outputs * (1 + seed_outputs)` — one primal plus one
///   tangent per seed output. Single-output seed (`seed_outputs = 1`) is the
///   common case and matches the C++ `getBoxType` behavior
///   (`boxtype.cpp:371`). Multi-output seeds bundle several independent
///   differentiation variables through a single `fad` node.
/// - `rad(expr, seeds)` reports `outputs = body_outputs + seed_outputs`:
///   primals first, then one gradient lane per seed output.
pub fn box_arity_typed(
    arena: &TreeArena,
    box_tree: FlatBoxId,
    cache: &mut ArityCache,
) -> Result<BoxArity, PropagateError> {
    if let Some(cached) = cache.typed.get(&box_tree) {
        return cached.clone();
    }
    let result = box_arity_flat_inner(arena, box_tree, cache);
    cache.typed.insert(box_tree, result.clone());
    result
}

/// Like [`box_arity_typed`] but treats `ForwardAD` as transparent (no output
/// expansion).
///
/// This is used only for the `RecFadMode::ExpandAfterRec` path, where the
/// recursive port algebra is computed on the primal lanes and the tangent
/// bundle is emitted after the recursive group has been finalized.
pub(crate) fn box_arity_wiring(
    arena: &TreeArena,
    box_tree: FlatBoxId,
    cache: &mut ArityCache,
) -> Result<BoxArity, PropagateError> {
    if let Some(cached) = cache.wiring.get(&box_tree) {
        return cached.clone();
    }
    let result = box_arity_wiring_inner(arena, box_tree, cache);
    cache.wiring.insert(box_tree, result.clone());
    result
}

/// Wiring arity on a cache miss: unwrap `ForwardAD` layers and walk the
/// composition nodes, delegating to [`box_arity_typed`] everywhere else,
/// where the typed arity is the wiring arity.
fn box_arity_wiring_inner(
    arena: &TreeArena,
    box_tree: FlatBoxId,
    cache: &mut ArityCache,
) -> Result<BoxArity, PropagateError> {
    match flat_node_kind(arena, box_tree)? {
        FlatNodeKind::ForwardAD { body, .. } => box_arity_wiring(arena, body, cache),
        FlatNodeKind::VGroup { body }
        | FlatNodeKind::HGroup { body }
        | FlatNodeKind::TGroup { body }
        | FlatNodeKind::Metadata { body } => {
            let inner = box_arity_wiring(arena, body, cache)?;
            Ok(inner)
        }
        FlatNodeKind::Symbolic { body } => {
            let inner = box_arity_wiring(arena, body, cache)?;
            Ok(BoxArity {
                inputs: inner.inputs + 1,
                outputs: inner.outputs,
            })
        }
        FlatNodeKind::Seq(left, right) => {
            let la = box_arity_wiring(arena, left, cache)?;
            let ra = box_arity_wiring(arena, right, cache)?;
            if la.outputs != ra.inputs {
                return Err(PropagateError::SeqArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: la.outputs,
                    right_inputs: ra.inputs,
                });
            }
            Ok(BoxArity {
                inputs: la.inputs,
                outputs: ra.outputs,
            })
        }
        FlatNodeKind::Par(left, right) => {
            let la = box_arity_wiring(arena, left, cache)?;
            let ra = box_arity_wiring(arena, right, cache)?;
            Ok(BoxArity {
                inputs: la.inputs + ra.inputs,
                outputs: la.outputs + ra.outputs,
            })
        }
        FlatNodeKind::Rec(left, right) => {
            let la = box_arity_wiring(arena, left, cache)?;
            let ra = box_arity_wiring(arena, right, cache)?;
            if ra.inputs > la.outputs || ra.outputs > la.inputs {
                return Err(PropagateError::RecArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_inputs: la.inputs,
                    left_outputs: la.outputs,
                    right_inputs: ra.inputs,
                    right_outputs: ra.outputs,
                });
            }
            Ok(BoxArity {
                inputs: la.inputs.saturating_sub(ra.outputs),
                outputs: la.outputs,
            })
        }
        FlatNodeKind::Split(left, right) => {
            let la = box_arity_wiring(arena, left, cache)?;
            let ra = box_arity_wiring(arena, right, cache)?;
            if !split_compatible(la.outputs, ra.inputs) {
                return Err(PropagateError::SplitArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: la.outputs,
                    right_inputs: ra.inputs,
                });
            }
            Ok(BoxArity {
                inputs: la.inputs,
                outputs: ra.outputs,
            })
        }
        FlatNodeKind::Merge(left, right) => {
            let la = box_arity_wiring(arena, left, cache)?;
            let ra = box_arity_wiring(arena, right, cache)?;
            if !merge_compatible(la.outputs, ra.inputs) {
                return Err(PropagateError::MergeArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: la.outputs,
                    right_inputs: ra.inputs,
                });
            }
            Ok(BoxArity {
                inputs: la.inputs,
                outputs: ra.outputs,
            })
        }
        // For all other node kinds, ForwardAD doesn't appear in the subtree
        // and the typed arity is identical to the wiring arity.
        _ => box_arity_typed(arena, box_tree, cache),
    }
}

/// Core arity inference logic, called only on cache miss.
fn box_arity_flat_inner(
    arena: &TreeArena,
    box_tree: FlatBoxId,
    cache: &mut ArityCache,
) -> Result<BoxArity, PropagateError> {
    match flat_node_kind(arena, box_tree)? {
        FlatNodeKind::Int | FlatNodeKind::Real => Ok(BoxArity {
            inputs: 0,
            outputs: 1,
        }),
        FlatNodeKind::Slot => Ok(BoxArity {
            inputs: 0,
            outputs: 1,
        }),
        FlatNodeKind::Metadata { body } => box_arity_typed(arena, body, cache),
        FlatNodeKind::Wire => Ok(BoxArity {
            inputs: 1,
            outputs: 1,
        }),
        FlatNodeKind::Cut => Ok(BoxArity {
            inputs: 1,
            outputs: 0,
        }),
        FlatNodeKind::Prim2 => Ok(BoxArity {
            inputs: 2,
            outputs: 1,
        }),
        FlatNodeKind::Prim1 => Ok(BoxArity {
            inputs: 1,
            outputs: 1,
        }),
        FlatNodeKind::Prim3 => Ok(BoxArity {
            inputs: 3,
            outputs: 1,
        }),
        FlatNodeKind::Prim4 => Ok(BoxArity {
            inputs: 4,
            outputs: 1,
        }),
        FlatNodeKind::Prim5 => Ok(BoxArity {
            inputs: 5,
            outputs: 1,
        }),
        FlatNodeKind::FConst | FlatNodeKind::FVar => Ok(BoxArity {
            inputs: 0,
            outputs: 1,
        }),
        FlatNodeKind::FFun => {
            let BoxMatch::FFun(ff) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat ffun node must decode to BoxMatch::FFun")
            };
            Ok(BoxArity {
                inputs: ffunction_arity(arena, ff)?,
                outputs: 1,
            })
        }
        FlatNodeKind::Button
        | FlatNodeKind::Checkbox
        | FlatNodeKind::VSlider
        | FlatNodeKind::HSlider
        | FlatNodeKind::NumEntry => Ok(BoxArity {
            inputs: 0,
            outputs: 1,
        }),
        FlatNodeKind::VBargraph | FlatNodeKind::HBargraph => Ok(BoxArity {
            inputs: 1,
            outputs: 1,
        }),
        FlatNodeKind::Soundfile => {
            let BoxMatch::Soundfile(_, chan) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat soundfile node must decode to BoxMatch::Soundfile")
            };
            let chan = usize_from_int_node(arena, chan, "soundfile channels")?;
            Ok(BoxArity {
                inputs: 2,
                outputs: 2 + chan,
            })
        }
        FlatNodeKind::Waveform => {
            let BoxMatch::Waveform(values) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat waveform node must decode to BoxMatch::Waveform")
            };
            let _ = list_length(arena, values).ok_or(PropagateError::UnsupportedBox {
                node: box_tree.as_tree_id(),
                kind: "waveform-list",
            })?;
            Ok(BoxArity {
                inputs: 0,
                outputs: 2,
            })
        }
        FlatNodeKind::VGroup { body }
        | FlatNodeKind::HGroup { body }
        | FlatNodeKind::TGroup { body } => box_arity_typed(arena, body, cache),
        FlatNodeKind::ReverseAD { body, seeds } => {
            let body_arity = box_arity_typed(arena, body, cache)?;
            if body_arity.outputs == 0 {
                return Err(PropagateError::RadBodyArity {
                    node: box_tree.as_tree_id(),
                    outputs: body_arity.outputs,
                });
            }
            let seeds_arity = box_arity_typed(arena, seeds, cache)?;
            if seeds_arity.outputs == 0 {
                return Err(PropagateError::RadSeedArity {
                    node: box_tree.as_tree_id(),
                    outputs: seeds_arity.outputs,
                });
            }
            Ok(BoxArity {
                inputs: body_arity.inputs.max(seeds_arity.inputs),
                outputs: body_arity.outputs + seeds_arity.outputs,
            })
        }
        FlatNodeKind::ForwardAD { body, seed } => {
            let seed_arity = box_arity_typed(arena, seed, cache)?;
            if seed_arity.outputs == 0 {
                return Err(PropagateError::FadSeedArity {
                    node: box_tree.as_tree_id(),
                    outputs: seed_arity.outputs,
                });
            }
            let inner = box_arity_typed(arena, body, cache)?;
            Ok(BoxArity {
                inputs: inner.inputs.max(seed_arity.inputs),
                outputs: inner.outputs * (1 + seed_arity.outputs),
            })
        }
        FlatNodeKind::Symbolic { body } => {
            let inner = box_arity_typed(arena, body, cache)?;
            Ok(BoxArity {
                inputs: inner.inputs + 1,
                outputs: inner.outputs,
            })
        }
        FlatNodeKind::Seq(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            if left_arity.outputs != right_arity.inputs {
                return Err(PropagateError::SeqArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            Ok(BoxArity {
                inputs: left_arity.inputs,
                outputs: right_arity.outputs,
            })
        }
        FlatNodeKind::Par(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            Ok(BoxArity {
                inputs: left_arity.inputs + right_arity.inputs,
                outputs: left_arity.outputs + right_arity.outputs,
            })
        }
        FlatNodeKind::Split(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            if !split_compatible(left_arity.outputs, right_arity.inputs) {
                return Err(PropagateError::SplitArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            Ok(BoxArity {
                inputs: left_arity.inputs,
                outputs: right_arity.outputs,
            })
        }
        FlatNodeKind::Merge(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            if !merge_compatible(left_arity.outputs, right_arity.inputs) {
                return Err(PropagateError::MergeArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            Ok(BoxArity {
                inputs: left_arity.inputs,
                outputs: right_arity.outputs,
            })
        }
        FlatNodeKind::Rec(left, right) => {
            let fad_mode = rec_fad_mode(arena, left, right, cache)?;
            let (left_arity, right_arity) = match fad_mode {
                RecFadMode::None | RecFadMode::ExpandAfterRec => (
                    box_arity_wiring(arena, left, cache)?,
                    box_arity_wiring(arena, right, cache)?,
                ),
                RecFadMode::AugmentedState => (
                    box_arity_typed(arena, left, cache)?,
                    box_arity_typed(arena, right, cache)?,
                ),
            };
            if right_arity.inputs > left_arity.outputs || right_arity.outputs > left_arity.inputs {
                return Err(PropagateError::RecArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_inputs: left_arity.inputs,
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                    right_outputs: right_arity.outputs,
                });
            }
            let core_outputs = left_arity.outputs;
            let core_inputs = left_arity.inputs.saturating_sub(right_arity.outputs);
            let outputs = match fad_mode {
                RecFadMode::None => core_outputs,
                RecFadMode::ExpandAfterRec => {
                    // One tangent bundle per `fad` application: a box both
                    // branches share is applied twice and seeds twice.
                    let mut memo = AHashMap::new();
                    let n_left = count_fad_nodes(arena, left, &mut memo)?;
                    let n_right = count_fad_nodes(arena, right, &mut memo)?;
                    core_outputs * (1 + n_left + n_right)
                }
                RecFadMode::AugmentedState => core_outputs,
            };
            Ok(BoxArity {
                inputs: core_inputs,
                outputs,
            })
        }
        FlatNodeKind::Environment => Ok(BoxArity {
            inputs: 0,
            outputs: 0,
        }),
        FlatNodeKind::Route => {
            let BoxMatch::Route(ins, outs, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat route node must decode to BoxMatch::Route")
            };
            Ok(BoxArity {
                inputs: usize_from_int_node(arena, ins, "route inputs")?,
                outputs: usize_from_int_node(arena, outs, "route outputs")?,
            })
        }
        FlatNodeKind::Inputs | FlatNodeKind::Outputs => Ok(BoxArity {
            inputs: 0,
            outputs: 1,
        }),
        FlatNodeKind::Ondemand(expr)
        | FlatNodeKind::Upsampling(expr)
        | FlatNodeKind::Downsampling(expr) => {
            let inner = box_arity_typed(arena, expr, cache)?;
            Ok(BoxArity {
                inputs: inner.inputs + 1,
                outputs: inner.outputs,
            })
        }
    }
}
