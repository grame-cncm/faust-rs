//! Operational box-to-signal propagation engine.
//!
//! The engine lowers validated flat boxes into signal trees while threading
//! slot bindings, recursion context, AD expansion state, and De Bruijn
//! placeholders. Public callers should use the APIs re-exported from `api`
//! rather than entering this module directly.

use super::*;

/// Propagates one box tree with an explicit slot environment.
///
/// Source provenance (C++):
/// - `compiler/propagate/propagate.cpp`
/// - `propagate(...)`
///
/// C++ threads a dedicated `slotenv` alongside the normal recursion so
/// `boxSymbolic(slot, body)` can bind the first input bus to `boxSlot(slot)`.
/// Rust keeps the same semantic mechanism but uses a local hash map keyed by the
/// canonical `BoxId` of each slot node instead of global tree properties.
///
/// This helper is also the point where Rust enforces the public
/// `propagate(...)` contract: callers may only enter `propagate_inner(...)`
/// through a path that has already checked both input and output bus widths.
pub(crate) fn propagate_in_slot_env(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    ctx: &mut PropagateContext<'_>,
) -> Result<Vec<SigId>, PropagateError> {
    let arity = box_arity_typed(arena, box_tree, ctx.cache)?;
    if inputs.len() != arity.inputs {
        return Err(PropagateError::InputArityMismatch {
            node: box_tree.as_tree_id(),
            expected: arity.inputs,
            got: inputs.len(),
        });
    }
    let outputs = propagate_inner(arena, box_tree, inputs, ctx)?;
    // Output arity validation: signal count may be less than box arity in two
    // cases:
    // 1. `suppress_fad` is active (Rec branch propagation): FAD expansion is
    //    deferred, so only primal outputs are produced.
    // 2. `[autodiff:false]` metadata: box-level control counting cannot see
    //    per-control metadata, so the box arity is an upper bound. The actual
    //    signal count may be lower when some controls are excluded.
    //
    // Outputs must never *exceed* the box arity prediction.
    if outputs.len() > arity.outputs {
        return Err(PropagateError::OutputArityMismatch {
            node: box_tree.as_tree_id(),
            expected: arity.outputs,
            got: outputs.len(),
        });
    }
    // When no FAD is involved, outputs must match exactly.
    if outputs.len() != arity.outputs && !ctx.suppress_fad && !contains_forward_ad(arena, box_tree)?
    {
        return Err(PropagateError::OutputArityMismatch {
            node: box_tree.as_tree_id(),
            expected: arity.outputs,
            got: outputs.len(),
        });
    }
    Ok(outputs)
}

/// Internal propagation dispatcher once input arity has been validated.
///
/// Unlike [`box_arity_typed`], this function is intentionally operational rather
/// than declarative: it builds actual signal nodes, threads slot bindings, and
/// recursively performs composition rewrites. Unsupported box families here are
/// therefore genuine lowering gaps, not just missing arity metadata.
fn propagate_inner(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    ctx: &mut PropagateContext<'_>,
) -> Result<Vec<SigId>, PropagateError> {
    match flat_node_kind(arena, box_tree)? {
        FlatNodeKind::Int => {
            let BoxMatch::Int(value) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat int node must decode to BoxMatch::Int")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.int(value)])
        }
        FlatNodeKind::Real => {
            let BoxMatch::Real(value) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat real node must decode to BoxMatch::Real")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.real(value)])
        }
        FlatNodeKind::Metadata { body } => propagate_inner(arena, body, inputs, ctx),
        FlatNodeKind::Slot => {
            let BoxMatch::Slot(id) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat slot node must decode to BoxMatch::Slot")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            if let Some(sig) = ctx.slot_env.get(&box_tree.as_tree_id()).copied() {
                Ok(vec![sig])
            } else {
                let mut b = SigBuilder::new(arena);
                Ok(vec![b.input(id)])
            }
        }
        FlatNodeKind::Wire => {
            expect_input_arity(box_tree.as_tree_id(), inputs, 1)?;
            Ok(vec![inputs[0]])
        }
        FlatNodeKind::Cut => {
            expect_input_arity(box_tree.as_tree_id(), inputs, 1)?;
            Ok(Vec::new())
        }
        FlatNodeKind::Prim2 => {
            let op = match_box(arena, box_tree.as_tree_id());
            match op {
                BoxMatch::Add => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.add(x, y))
                }
                BoxMatch::Sub => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.sub(x, y))
                }
                BoxMatch::Mul => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.mul(x, y))
                }
                BoxMatch::Div => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.div(x, y))
                }
                BoxMatch::Rem => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.rem(x, y))
                }
                BoxMatch::And => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.and(x, y))
                }
                BoxMatch::Or => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.or(x, y))
                }
                BoxMatch::Xor => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.xor(x, y))
                }
                BoxMatch::Lsh => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.lsh(x, y))
                }
                BoxMatch::Rsh => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.arsh(x, y))
                }
                BoxMatch::LRsh => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.lrsh(x, y))
                }
                BoxMatch::Lt => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.lt(x, y))
                }
                BoxMatch::Le => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.le(x, y))
                }
                BoxMatch::Gt => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.gt(x, y))
                }
                BoxMatch::Ge => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.ge(x, y))
                }
                BoxMatch::Eq => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.eq(x, y))
                }
                BoxMatch::Ne => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.ne(x, y))
                }
                BoxMatch::Pow => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.pow(x, y))
                }
                BoxMatch::Atan2 => binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                    b.atan2(x, y)
                }),
                BoxMatch::Fmod => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.fmod(x, y))
                }
                BoxMatch::Remainder => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                        b.remainder(x, y)
                    })
                }
                BoxMatch::Min => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.min(x, y))
                }
                BoxMatch::Max => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.max(x, y))
                }
                BoxMatch::Delay => binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                    b.delay(x, y)
                }),
                BoxMatch::Prefix => binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                    b.prefix(x, y)
                }),
                BoxMatch::Attach => binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                    b.attach(x, y)
                }),
                BoxMatch::Enable => binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                    b.enable(x, y)
                }),
                BoxMatch::Control => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                        b.control(x, y)
                    })
                }
                _ => unreachable!("flat prim2 node must decode to a binary primitive"),
            }
        }
        FlatNodeKind::Prim1 => {
            let op = match_box(arena, box_tree.as_tree_id());
            match op {
                BoxMatch::Delay1 => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.delay1(x))
                }
                BoxMatch::IntCast => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.int_cast(x))
                }
                BoxMatch::FloatCast => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.float_cast(x))
                }
                BoxMatch::Acos => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.acos(x))
                }
                BoxMatch::Asin => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.asin(x))
                }
                BoxMatch::Atan => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.atan(x))
                }
                BoxMatch::Cos => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.cos(x)),
                BoxMatch::Sin => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.sin(x)),
                BoxMatch::Tan => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.tan(x)),
                BoxMatch::Exp => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.exp(x)),
                BoxMatch::Exp10 => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.exp10(x))
                }
                BoxMatch::Log => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.log(x)),
                BoxMatch::Log10 => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.log10(x))
                }
                BoxMatch::Sqrt => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.sqrt(x))
                }
                BoxMatch::Abs => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.abs(x)),
                BoxMatch::Floor => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.floor(x))
                }
                BoxMatch::Ceil => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.ceil(x))
                }
                BoxMatch::Rint => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.rint(x))
                }
                BoxMatch::Round => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.round(x))
                }
                BoxMatch::Lowest => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.lowest(x))
                }
                BoxMatch::Highest => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.highest(x))
                }
                _ => unreachable!("flat prim1 node must decode to a unary primitive"),
            }
        }
        FlatNodeKind::Prim3 => {
            let op = match_box(arena, box_tree.as_tree_id());
            match op {
                BoxMatch::ReadOnlyTable => {
                    ternary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y, z| {
                        b.read_only_table(x, y, z)
                    })
                }
                BoxMatch::Select2 => {
                    ternary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y, z| {
                        b.select2(x, y, z)
                    })
                }
                BoxMatch::AssertBounds => {
                    ternary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y, z| {
                        b.assert_bounds(x, y, z)
                    })
                }
                _ => unreachable!("flat prim3 node must decode to a ternary primitive"),
            }
        }
        FlatNodeKind::Prim4 => {
            quaternary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y, z, w| {
                b.select3(x, y, z, w)
            })
        }
        FlatNodeKind::Prim5 => quinary_prim(
            arena,
            box_tree.as_tree_id(),
            inputs,
            |b, s, i, wi, ws, ri| b.write_read_table(s, i, wi, ws, ri),
        ),
        FlatNodeKind::FConst => {
            let BoxMatch::FConst(ty, name, file) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat fconst node must decode to BoxMatch::FConst")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.fconst(ty, name, file)])
        }
        FlatNodeKind::FVar => {
            let BoxMatch::FVar(ty, name, file) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat fvar node must decode to BoxMatch::FVar")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.fvar(ty, name, file)])
        }
        FlatNodeKind::Button => {
            let BoxMatch::Button(_) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat button node must decode to BoxMatch::Button")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let ctx_hash = group_path_hash(&ctx.current_groups);
            let control = *ctx
                .control_ids
                .get(&(box_tree.as_tree_id(), ctx_hash))
                .expect("button control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.button(control)])
        }
        FlatNodeKind::Checkbox => {
            let BoxMatch::Checkbox(_) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat checkbox node must decode to BoxMatch::Checkbox")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let ctx_hash = group_path_hash(&ctx.current_groups);
            let control = *ctx
                .control_ids
                .get(&(box_tree.as_tree_id(), ctx_hash))
                .expect("checkbox control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.checkbox(control)])
        }
        FlatNodeKind::VSlider => {
            let BoxMatch::VSlider(_, _, _, _, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat vslider node must decode to BoxMatch::VSlider")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let ctx_hash = group_path_hash(&ctx.current_groups);
            let control = *ctx
                .control_ids
                .get(&(box_tree.as_tree_id(), ctx_hash))
                .expect("vslider control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.vslider(control)])
        }
        FlatNodeKind::HSlider => {
            let BoxMatch::HSlider(_, _, _, _, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat hslider node must decode to BoxMatch::HSlider")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let ctx_hash = group_path_hash(&ctx.current_groups);
            let control = *ctx
                .control_ids
                .get(&(box_tree.as_tree_id(), ctx_hash))
                .expect("hslider control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.hslider(control)])
        }
        FlatNodeKind::NumEntry => {
            let BoxMatch::NumEntry(_, _, _, _, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat numentry node must decode to BoxMatch::NumEntry")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let ctx_hash = group_path_hash(&ctx.current_groups);
            let control = *ctx
                .control_ids
                .get(&(box_tree.as_tree_id(), ctx_hash))
                .expect("numentry control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.numentry(control)])
        }
        FlatNodeKind::VBargraph => {
            let BoxMatch::VBargraph(_, _, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat vbargraph node must decode to BoxMatch::VBargraph")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 1)?;
            let ctx_hash = group_path_hash(&ctx.current_groups);
            let control = *ctx
                .control_ids
                .get(&(box_tree.as_tree_id(), ctx_hash))
                .expect("vbargraph control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.vbargraph(control, inputs[0])])
        }
        FlatNodeKind::HBargraph => {
            let BoxMatch::HBargraph(_, _, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat hbargraph node must decode to BoxMatch::HBargraph")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 1)?;
            let ctx_hash = group_path_hash(&ctx.current_groups);
            let control = *ctx
                .control_ids
                .get(&(box_tree.as_tree_id(), ctx_hash))
                .expect("hbargraph control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.hbargraph(control, inputs[0])])
        }
        FlatNodeKind::Waveform => {
            let BoxMatch::Waveform(values) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat waveform node must decode to BoxMatch::Waveform")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let values = list_to_vec(arena, values).ok_or(PropagateError::UnsupportedBox {
                node: box_tree.as_tree_id(),
                kind: "waveform-list",
            })?;
            let mut b = SigBuilder::new(arena);
            let n = i32_from_usize(values.len(), "waveform size")?;
            let size = b.int(n);
            let waveform = b.waveform(&values);
            Ok(vec![size, waveform])
        }
        FlatNodeKind::VGroup { body } => {
            let label = match match_box(arena, box_tree.as_tree_id()) {
                BoxMatch::VGroup(label, _) => decode_box_label(arena, label),
                _ => unreachable!("flat vgroup node must decode to BoxMatch::VGroup"),
            };
            let UiNormalizedGroupPath {
                mut parent_groups,
                group,
            } = normalize_group_label_navigation(
                &label,
                &ctx.current_groups,
                UiGroupKind::Vertical,
            );
            parent_groups.push(group);
            let saved = std::mem::replace(&mut ctx.current_groups, parent_groups);
            let result = propagate_in_slot_env(arena, body, inputs, ctx);
            ctx.current_groups = saved;
            result
        }
        FlatNodeKind::HGroup { body } => {
            let label = match match_box(arena, box_tree.as_tree_id()) {
                BoxMatch::HGroup(label, _) => decode_box_label(arena, label),
                _ => unreachable!("flat hgroup node must decode to BoxMatch::HGroup"),
            };
            let UiNormalizedGroupPath {
                mut parent_groups,
                group,
            } = normalize_group_label_navigation(
                &label,
                &ctx.current_groups,
                UiGroupKind::Horizontal,
            );
            parent_groups.push(group);
            let saved = std::mem::replace(&mut ctx.current_groups, parent_groups);
            let result = propagate_in_slot_env(arena, body, inputs, ctx);
            ctx.current_groups = saved;
            result
        }
        FlatNodeKind::TGroup { body } => {
            let label = match match_box(arena, box_tree.as_tree_id()) {
                BoxMatch::TGroup(label, _) => decode_box_label(arena, label),
                _ => unreachable!("flat tgroup node must decode to BoxMatch::TGroup"),
            };
            let UiNormalizedGroupPath {
                mut parent_groups,
                group,
            } = normalize_group_label_navigation(&label, &ctx.current_groups, UiGroupKind::Tab);
            parent_groups.push(group);
            let saved = std::mem::replace(&mut ctx.current_groups, parent_groups);
            let result = propagate_in_slot_env(arena, body, inputs, ctx);
            ctx.current_groups = saved;
            result
        }
        FlatNodeKind::Symbolic { body } => {
            let BoxMatch::Symbolic(slot, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat symbolic node must decode to BoxMatch::Symbolic")
            };
            if inputs.is_empty() {
                return Err(PropagateError::InputArityMismatch {
                    node: box_tree.as_tree_id(),
                    expected: 1,
                    got: 0,
                });
            }
            let previous = ctx.slot_env.insert(slot, inputs[0]);
            let result = propagate_in_slot_env(arena, body, &inputs[1..], ctx);
            if let Some(sig) = previous {
                ctx.slot_env.insert(slot, sig);
            } else {
                ctx.slot_env.remove(&slot);
            }
            result
        }
        FlatNodeKind::Seq(left, right) => {
            let left_arity = box_arity_typed(arena, left, ctx.cache)?;
            let right_arity = box_arity_typed(arena, right, ctx.cache)?;
            if left_arity.outputs != right_arity.inputs {
                return Err(PropagateError::SeqArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            let mid = propagate_in_slot_env(arena, left, inputs, ctx)?;
            propagate_in_slot_env(arena, right, &mid, ctx)
        }
        FlatNodeKind::Par(left, right) => {
            let left_arity = box_arity_typed(arena, left, ctx.cache)?;
            let right_arity = box_arity_typed(arena, right, ctx.cache)?;
            let left_out = propagate_in_slot_env(arena, left, &inputs[..left_arity.inputs], ctx)?;
            let mut right_out = propagate_in_slot_env(
                arena,
                right,
                &inputs[left_arity.inputs..left_arity.inputs + right_arity.inputs],
                ctx,
            )?;
            let mut out = left_out;
            out.append(&mut right_out);
            Ok(out)
        }
        FlatNodeKind::Split(left, right) => {
            let left_arity = box_arity_typed(arena, left, ctx.cache)?;
            let right_arity = box_arity_typed(arena, right, ctx.cache)?;
            if !split_compatible(left_arity.outputs, right_arity.inputs) {
                return Err(PropagateError::SplitArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            let left_out = propagate_in_slot_env(arena, left, inputs, ctx)?;
            let split_in = split_signals(&left_out, right_arity.inputs);
            propagate_in_slot_env(arena, right, &split_in, ctx)
        }
        FlatNodeKind::Merge(left, right) => {
            let left_arity = box_arity_typed(arena, left, ctx.cache)?;
            let right_arity = box_arity_typed(arena, right, ctx.cache)?;
            if !merge_compatible(left_arity.outputs, right_arity.inputs) {
                return Err(PropagateError::MergeArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            let left_out = propagate_in_slot_env(arena, left, inputs, ctx)?;
            let merge_in = mix_signals(arena, &left_out, right_arity.inputs);
            propagate_in_slot_env(arena, right, &merge_in, ctx)
        }
        FlatNodeKind::Rec(left, right) => {
            let fad_mode = rec_fad_mode(arena, left, right)?;
            let (left_arity, right_arity) = match fad_mode {
                RecFadMode::None | RecFadMode::ExpandAfterRec => (
                    box_arity_wiring(arena, left, ctx.cache)?,
                    box_arity_wiring(arena, right, ctx.cache)?,
                ),
                RecFadMode::AugmentedState => (
                    box_arity_typed(arena, left, ctx.cache)?,
                    box_arity_typed(arena, right, ctx.cache)?,
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

            let saved_suppress = ctx.suppress_fad;
            let saved_pending = std::mem::take(&mut ctx.pending_fad_seeds);
            if matches!(fad_mode, RecFadMode::ExpandAfterRec) {
                ctx.suppress_fad = true;
            }

            // Lift all slot_env values by 1 de Bruijn level before entering this Rec scope.
            // Both `right` (feedback branch) and `left` (main body) are semantically evaluated
            // inside the new Rec binder — their result signals live under a fresh `DEBRUIJNREC`
            // where `DEBRUIJNREF(1)` is the inner group. Any slot lookup that reaches a signal
            // built in the enclosing scope must therefore be shifted by one level so its
            // references still point to the intended outer binder after the new `DEBRUIJNREC`
            // is inserted. This must happen BEFORE propagating `right`, because `right` produces
            // `l1`, which is spliced into the inner body as part of `rec_inputs`.
            let lifted_slot_env: SlotEnv = ctx
                .slot_env
                .iter()
                .map(|(k, v)| (*k, liftn(arena, *v, 1, ctx.memo)))
                .collect();
            let saved_slot_env = std::mem::replace(ctx.slot_env, lifted_slot_env);

            let l0 = make_mem_sig_proj_list(arena, right_arity.inputs)?;
            let l1 = propagate_in_slot_env(arena, right, &l0, ctx)?;

            let mut rec_inputs = l1;
            rec_inputs.extend(lift_signals(arena, inputs, ctx.memo));

            let l2 = propagate_in_slot_env(arena, left, &rec_inputs, ctx)?;

            *ctx.slot_env = saved_slot_env;
            ctx.suppress_fad = saved_suppress;
            let seeds = std::mem::replace(&mut ctx.pending_fad_seeds, saved_pending);

            let group_body = vec_to_list(arena, &l2);
            let group = debruijn_rec(arena, group_body);

            let mut outputs = Vec::with_capacity(l2.len());
            for (index, expr) in l2.iter().copied().enumerate() {
                let ap = de_bruijn_aperture_with_memo(arena, expr, &mut ctx.memo.aperture);
                if ap > 0 {
                    let idx = i32_from_usize(index, "rec projection index")?;
                    let mut b = SigBuilder::new(arena);
                    outputs.push(b.proj(idx, group));
                } else {
                    outputs.push(expr);
                }
            }

            if matches!(fad_mode, RecFadMode::ExpandAfterRec) {
                forward_ad::generate_fad_signals_multi(arena, &outputs, &seeds)
            } else {
                Ok(outputs)
            }
        }
        FlatNodeKind::Inputs => {
            let BoxMatch::Inputs(expr) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat inputs node must decode to BoxMatch::Inputs")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let flat_expr = try_build_flat_box(arena, expr)?;
            let arity = box_arity_typed(arena, flat_expr, ctx.cache)?;
            let value = i32_from_usize(arity.inputs, "inputs")?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.int(value)])
        }
        FlatNodeKind::Outputs => {
            let BoxMatch::Outputs(expr) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat outputs node must decode to BoxMatch::Outputs")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let flat_expr = try_build_flat_box(arena, expr)?;
            let arity = box_arity_typed(arena, flat_expr, ctx.cache)?;
            let value = i32_from_usize(arity.outputs, "outputs")?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.int(value)])
        }
        FlatNodeKind::ForwardAD { body, seed } => {
            let body_arity = box_arity_typed(arena, body, ctx.cache)?;
            let seed_arity = box_arity_typed(arena, seed, ctx.cache)?;
            if seed_arity.outputs == 0 {
                return Err(PropagateError::FadSeedArity {
                    node: box_tree.as_tree_id(),
                    outputs: 0,
                });
            }
            let seed_inputs: Vec<SigId> = inputs.iter().copied().take(seed_arity.inputs).collect();
            // Seeds are registered without group context in collect_ui_nodes; reset the
            // group stack so that widget lookups here use the same (empty-context) key.
            let saved_groups = std::mem::take(&mut ctx.current_groups);
            let seed_sigs = propagate_in_slot_env(arena, seed, &seed_inputs, ctx)?;
            ctx.current_groups = saved_groups;
            if seed_sigs.len() != seed_arity.outputs {
                return Err(PropagateError::FadSeedArity {
                    node: box_tree.as_tree_id(),
                    outputs: seed_sigs.len(),
                });
            }
            let body_inputs: Vec<SigId> = inputs.iter().copied().take(body_arity.inputs).collect();
            let body_sigs = propagate_in_slot_env(arena, body, &body_inputs, ctx)?;
            if ctx.suppress_fad {
                ctx.pending_fad_seeds.extend(seed_sigs.iter().copied());
                Ok(body_sigs)
            } else {
                forward_ad::generate_fad_signals_multi(arena, &body_sigs, &seed_sigs)
            }
        }
        FlatNodeKind::ReverseAD { body, seeds } => {
            let body_arity = box_arity_typed(arena, body, ctx.cache)?;
            if body_arity.outputs == 0 {
                return Err(PropagateError::RadBodyArity {
                    node: box_tree.as_tree_id(),
                    outputs: 0,
                });
            }
            let seeds_arity = box_arity_typed(arena, seeds, ctx.cache)?;
            if seeds_arity.outputs == 0 {
                return Err(PropagateError::RadSeedArity {
                    node: box_tree.as_tree_id(),
                    outputs: 0,
                });
            }
            // Both children observe the same upstream input bus (mirrors the
            // FAD wiring contract).
            let seed_inputs: Vec<SigId> = inputs.iter().copied().take(seeds_arity.inputs).collect();
            // Same rationale as ForwardAD: seeds are registered without group context.
            let saved_groups = std::mem::take(&mut ctx.current_groups);
            let seed_sigs = propagate_in_slot_env(arena, seeds, &seed_inputs, ctx)?;
            ctx.current_groups = saved_groups;
            if seed_sigs.len() != seeds_arity.outputs {
                return Err(PropagateError::RadSeedArity {
                    node: box_tree.as_tree_id(),
                    outputs: seed_sigs.len(),
                });
            }
            let body_inputs: Vec<SigId> = inputs.iter().copied().take(body_arity.inputs).collect();
            let body_sigs = propagate_in_slot_env(arena, body, &body_inputs, ctx)?;
            if body_sigs.len() != body_arity.outputs {
                return Err(PropagateError::RadBodyArity {
                    node: box_tree.as_tree_id(),
                    outputs: body_sigs.len(),
                });
            }
            reverse_ad::generate_rad_signals(arena, &body_sigs, &seed_sigs)
        }
        FlatNodeKind::Environment => {
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            Ok(Vec::new())
        }
        FlatNodeKind::Route => {
            let BoxMatch::Route(ins, outs, route_spec) = match_box(arena, box_tree.as_tree_id())
            else {
                unreachable!("flat route node must decode to BoxMatch::Route")
            };
            let input_count = usize_from_int_node(arena, ins, "route inputs")?;
            let output_count = usize_from_int_node(arena, outs, "route outputs")?;
            expect_input_arity(box_tree.as_tree_id(), inputs, input_count)?;

            let route = flatten_route_ints(arena, route_spec)?;
            let mut b = SigBuilder::new(arena);
            let mut outputs: Vec<Option<SigId>> = vec![None; output_count];

            // Validate index helper
            fn to_valid_index(channel: i64, len: usize) -> Option<usize> {
                let index = usize::try_from(channel.checked_sub(1)?).ok()?;
                (index < len).then_some(index)
            }

            // route propagation
            for pair in route.chunks_exact(2) {
                let src_channel = pair[0];
                let dst_channel = pair[1];

                let Some(src_index) = to_valid_index(src_channel, input_count) else {
                    continue;
                };
                let Some(dst_index) = to_valid_index(dst_channel, output_count) else {
                    continue;
                };

                // valid source and destination
                let src_signal = inputs[src_index];
                let dst_signal = outputs[dst_index];

                outputs[dst_index] = Some(match dst_signal {
                    Some(existing) => b.add(existing, src_signal),
                    None => src_signal,
                });
            }

            let zero = b.int(0);
            Ok(outputs.into_iter().map(|sig| sig.unwrap_or(zero)).collect())
        }
        FlatNodeKind::FFun => {
            let BoxMatch::FFun(ff) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat ffun node must decode to BoxMatch::FFun")
            };
            let expected = ffunction_arity(arena, ff)?;
            expect_input_arity(box_tree.as_tree_id(), inputs, expected)?;
            let args = vec_to_list(arena, inputs);
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.ffun(ff, args)])
        }
        FlatNodeKind::Soundfile => {
            let BoxMatch::Soundfile(_, chan) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat soundfile node must decode to BoxMatch::Soundfile")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 2)?;
            let chan_count = usize_from_int_node(arena, chan, "soundfile channels")?;
            let mut b = SigBuilder::new(arena);
            let ctx_hash = group_path_hash(&ctx.current_groups);
            let control = *ctx
                .control_ids
                .get(&(box_tree.as_tree_id(), ctx_hash))
                .expect("soundfile control id must be registered during UI extraction");
            let soundfile = b.soundfile(control);
            let part = inputs[0];
            let length = b.soundfile_length(soundfile, part);
            let rate = b.soundfile_rate(soundfile, part);
            let one = b.int(1);
            let zero = b.int(0);
            let upper = b.sub(length, one);
            let limited = b.min(inputs[1], upper);
            let clamped = b.max(zero, limited);

            let mut outputs = Vec::with_capacity(chan_count + 2);
            outputs.push(length);
            outputs.push(rate);
            for chan_index in 0..chan_count {
                let chan_sig = b.int(i32_from_usize(chan_index, "soundfile buffer channel")?);
                outputs.push(b.soundfile_buffer(soundfile, chan_sig, part, clamped));
            }
            Ok(outputs)
        }
        FlatNodeKind::Ondemand(body) => propagate_clocked_wrapper(
            arena,
            box_tree,
            body,
            inputs,
            ctx,
            ClockedWrapperKind::Ondemand,
        ),
        FlatNodeKind::Upsampling(body) => propagate_clocked_wrapper(
            arena,
            box_tree,
            body,
            inputs,
            ctx,
            ClockedWrapperKind::Upsampling,
        ),
        FlatNodeKind::Downsampling(body) => propagate_clocked_wrapper(
            arena,
            box_tree,
            body,
            inputs,
            ctx,
            ClockedWrapperKind::Downsampling,
        ),
    }
}

#[derive(Clone, Copy)]
/// Clocked-wrapper categories recognized during propagation.
enum ClockedWrapperKind {
    Ondemand,
    Upsampling,
    Downsampling,
}

fn propagate_clocked_wrapper(
    arena: &mut TreeArena,
    wrapper_node: FlatBoxId,
    body: FlatBoxId,
    inputs: &[SigId],
    ctx: &mut PropagateContext<'_>,
    kind: ClockedWrapperKind,
) -> Result<Vec<SigId>, PropagateError> {
    let Some((&clock, tail)) = inputs.split_first() else {
        return Err(PropagateError::InputArityMismatch {
            node: wrapper_node.as_tree_id(),
            expected: 1,
            got: 0,
        });
    };

    let body_arity = box_arity_typed(arena, body, ctx.cache)?;
    if is_const_zero(arena, clock) {
        let mut b = SigBuilder::new(arena);
        let zero = b.int(0);
        return Ok(vec![zero; body_arity.outputs]);
    }
    if is_const_one(arena, clock) {
        return propagate_in_slot_env(arena, body, tail, ctx);
    }

    let clock_env = ctx.clock_env;
    let domain_kind = match kind {
        ClockedWrapperKind::Ondemand => ClockDomainKind::OnDemand,
        ClockedWrapperKind::Upsampling => ClockDomainKind::Upsampling,
        ClockedWrapperKind::Downsampling => ClockDomainKind::Downsampling,
    };
    let (domain_id, clock_env2) = make_clock_env(
        arena,
        ctx,
        domain_kind,
        wrapper_node.as_tree_id(),
        clock,
        tail,
    );
    let x1: Vec<_> = {
        let mut b = SigBuilder::new(arena);
        tail.iter().copied().map(|sig| b.temp_var(sig)).collect()
    };
    let x2: Vec<_> = {
        let mut b = SigBuilder::new(arena);
        x1.iter()
            .copied()
            .map(|sig| {
                let clocked = b.double_clocked(clock_env2, clock_env, sig);
                match kind {
                    ClockedWrapperKind::Ondemand | ClockedWrapperKind::Downsampling => clocked,
                    ClockedWrapperKind::Upsampling => b.zero_pad(clocked, clock),
                }
            })
            .collect()
    };
    let parent_clock_env = ctx.clock_env;
    let parent_clock_domain = ctx.clock_domain;
    ctx.clock_env = clock_env2;
    ctx.clock_domain = Some(domain_id);
    let y0 = propagate_in_slot_env(arena, body, &x2, ctx)?;
    ctx.clock_env = parent_clock_env;
    ctx.clock_domain = parent_clock_domain;

    let y1: Vec<_> = {
        let mut b = SigBuilder::new(arena);
        y0.iter()
            .copied()
            .map(|sig| {
                let clocked_sig = b.clocked(clock_env2, sig);
                b.perm_var(clocked_sig)
            })
            .collect()
    };
    let wrapper = {
        let mut b = SigBuilder::new(arena);
        let clocked_clock = b.clocked(clock_env2, clock);
        let mut wrapper_payload = Vec::with_capacity(y1.len() + 1);
        wrapper_payload.push(clocked_clock);
        wrapper_payload.extend(y1.iter().copied());
        match kind {
            ClockedWrapperKind::Ondemand => b.on_demand(&wrapper_payload),
            ClockedWrapperKind::Upsampling => b.upsampling(&wrapper_payload),
            ClockedWrapperKind::Downsampling => b.downsampling(&wrapper_payload),
        }
    };

    let mut b = SigBuilder::new(arena);
    Ok(y1.into_iter().map(|sig| b.seq(wrapper, sig)).collect())
}

fn is_const_zero(arena: &TreeArena, sig: SigId) -> bool {
    match match_sig(arena, sig) {
        SigMatch::Int(value) => value == 0,
        SigMatch::Real(value) => value == 0.0,
        _ => false,
    }
}

fn is_const_one(arena: &TreeArena, sig: SigId) -> bool {
    match match_sig(arena, sig) {
        SigMatch::Int(value) => value == 1,
        SigMatch::Real(value) => value == 1.0,
        _ => false,
    }
}

/// Validates that a primitive receives exactly the expected number of inputs.
fn expect_input_arity(
    node: TreeId,
    inputs: &[SigId],
    expected: usize,
) -> Result<(), PropagateError> {
    if inputs.len() == expected {
        Ok(())
    } else {
        Err(PropagateError::InputArityMismatch {
            node,
            expected,
            got: inputs.len(),
        })
    }
}

/// Lowers one unary primitive and returns a single output signal.
fn unary_prim(
    arena: &mut TreeArena,
    node: TreeId,
    inputs: &[SigId],
    f: impl FnOnce(&mut SigBuilder<'_>, SigId) -> SigId,
) -> Result<Vec<SigId>, PropagateError> {
    expect_input_arity(node, inputs, 1)?;
    let mut b = SigBuilder::new(arena);
    Ok(vec![f(&mut b, inputs[0])])
}

/// Lowers one binary primitive and returns a single output signal.
fn binary_prim(
    arena: &mut TreeArena,
    node: TreeId,
    inputs: &[SigId],
    f: impl FnOnce(&mut SigBuilder<'_>, SigId, SigId) -> SigId,
) -> Result<Vec<SigId>, PropagateError> {
    expect_input_arity(node, inputs, 2)?;
    let mut b = SigBuilder::new(arena);
    Ok(vec![f(&mut b, inputs[0], inputs[1])])
}

/// Lowers one ternary primitive and returns a single output signal.
fn ternary_prim(
    arena: &mut TreeArena,
    node: TreeId,
    inputs: &[SigId],
    f: impl FnOnce(&mut SigBuilder<'_>, SigId, SigId, SigId) -> SigId,
) -> Result<Vec<SigId>, PropagateError> {
    expect_input_arity(node, inputs, 3)?;
    let mut b = SigBuilder::new(arena);
    Ok(vec![f(&mut b, inputs[0], inputs[1], inputs[2])])
}

/// Lowers one quaternary primitive and returns a single output signal.
fn quaternary_prim(
    arena: &mut TreeArena,
    node: TreeId,
    inputs: &[SigId],
    f: impl FnOnce(&mut SigBuilder<'_>, SigId, SigId, SigId, SigId) -> SigId,
) -> Result<Vec<SigId>, PropagateError> {
    expect_input_arity(node, inputs, 4)?;
    let mut b = SigBuilder::new(arena);
    Ok(vec![f(&mut b, inputs[0], inputs[1], inputs[2], inputs[3])])
}

/// Lowers one quinary primitive and returns a single output signal.
fn quinary_prim(
    arena: &mut TreeArena,
    node: TreeId,
    inputs: &[SigId],
    f: impl FnOnce(&mut SigBuilder<'_>, SigId, SigId, SigId, SigId, SigId) -> SigId,
) -> Result<Vec<SigId>, PropagateError> {
    expect_input_arity(node, inputs, 5)?;
    let mut b = SigBuilder::new(arena);
    Ok(vec![f(
        &mut b, inputs[0], inputs[1], inputs[2], inputs[3], inputs[4],
    )])
}

/// Returns whether `split` wiring law is satisfied.
///
/// C++ parity rule:
/// - exact match, or
/// - right inputs is an integer multiple of left outputs.
pub(crate) fn split_compatible(left_outputs: usize, right_inputs: usize) -> bool {
    (left_outputs == right_inputs)
        || (left_outputs != 0 && right_inputs.is_multiple_of(left_outputs))
}

/// Returns whether `merge` wiring law is satisfied.
///
/// C++ parity rule:
/// - exact match, or
/// - left outputs is an integer multiple of right inputs.
pub(crate) fn merge_compatible(left_outputs: usize, right_inputs: usize) -> bool {
    (left_outputs == right_inputs)
        || (right_inputs != 0 && left_outputs.is_multiple_of(right_inputs))
}

/// Replicates input buses cyclically to feed `split` right-side arity.
fn split_signals(inputs: &[SigId], nbus: usize) -> Vec<SigId> {
    if nbus == 0 || inputs.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(nbus);
    for b in 0..nbus {
        out.push(inputs[b % inputs.len()]);
    }
    out
}

/// Mixes grouped buses by summing channels modulo `nbus` (merge semantics).
fn mix_signals(arena: &mut TreeArena, inputs: &[SigId], nbus: usize) -> Vec<SigId> {
    if nbus == 0 {
        return Vec::new();
    }

    let mut b = SigBuilder::new(arena);
    let mut out = Vec::with_capacity(nbus);

    for bus in 0..nbus {
        let mut acc = if bus < inputs.len() {
            inputs[bus]
        } else {
            b.int(0)
        };
        let mut idx = bus + nbus;
        while idx < inputs.len() {
            acc = b.add(acc, inputs[idx]);
            idx += nbus;
        }
        out.push(acc);
    }

    out
}

/// Returns list length for a `cons`/`nil` encoded list.
pub(crate) fn list_length(arena: &TreeArena, mut list: TreeId) -> Option<usize> {
    let mut len = 0usize;
    while !arena.is_nil(list) {
        let _ = arena.hd(list)?;
        list = arena.tl(list)?;
        len = len.checked_add(1)?;
    }
    Some(len)
}

/// Allocates one fresh clock-domain instance and returns its opaque in-graph token.
///
/// C++ stores `(parent, slotenv, path, box, inputs...)` as a tree list, where
/// `slotenv` + `path` provide instance uniqueness. Rust replaces that cons
/// tuple with a [`ClockDomainTable`] side arena (roadmap P0.2, plan §5.3):
/// each call allocates a fresh [`ClockDomain`] entry — the id is the
/// uniqueness token — and only the `SIGCLOCKENV` leaf carrying that id is
/// embedded in the signal graph. Two structurally identical wrapper instances
/// therefore always get distinct domains, closing the C++ de Bruijn collision
/// class (plan §3.4) by construction.
fn make_clock_env(
    arena: &mut TreeArena,
    ctx: &mut PropagateContext<'_>,
    kind: ClockDomainKind,
    box_node: TreeId,
    clock: SigId,
    inputs: &[SigId],
) -> (ClockDomainId, SigId) {
    let domain_id = ctx.clock_domains.alloc(ClockDomain {
        parent: ctx.clock_domain,
        kind,
        clock,
        wrapper_box: box_node,
        inputs: inputs.to_vec(),
    });
    let token = SigBuilder::new(arena).clock_env_token(domain_id.as_u32());
    (domain_id, token)
}

/// Flattens a route specification encoded as nested `par(...)` pairs into integer endpoints.
///
/// This mirrors the C++ `flattenRouteList(...)` helper used before `route`
/// propagation. The function only validates the already-built structural
/// payload; it does not normalize or evaluate the route expression.
fn flatten_route_ints(arena: &TreeArena, route_spec: TreeId) -> Result<Vec<i64>, PropagateError> {
    let mut out = Vec::new();
    flatten_route_ints_into(arena, route_spec, &mut out)?;
    Ok(out)
}

fn flatten_route_ints_into(
    arena: &TreeArena,
    node: TreeId,
    out: &mut Vec<i64>,
) -> Result<(), PropagateError> {
    match match_box(arena, node) {
        BoxMatch::Par(left, right) => {
            flatten_route_ints_into(arena, left, out)?;
            flatten_route_ints_into(arena, right, out)?;
            Ok(())
        }
        _ => {
            let Some(value) = tree_to_int(arena, node) else {
                return Err(PropagateError::UnsupportedBox {
                    node,
                    kind: "route-spec",
                });
            };
            out.push(value);
            Ok(())
        }
    }
}

/// Reads a non-negative integer node and converts it to `usize`.
pub(crate) fn usize_from_int_node(
    arena: &TreeArena,
    node: TreeId,
    field: &'static str,
) -> Result<usize, PropagateError> {
    let Some(value) = tree_to_int(arena, node) else {
        return Err(PropagateError::InvalidIntegerValue { node, field });
    };
    if value < 0 {
        return Err(PropagateError::NegativeIntegerValue { field, value });
    }
    usize::try_from(value).map_err(|_| PropagateError::InvalidIntegerValue { node, field })
}

/// Returns the C++ `ffarity(...)` for one wrapped foreign function descriptor.
pub(crate) fn ffunction_arity(arena: &TreeArena, ff: TreeId) -> Result<usize, PropagateError> {
    let BoxMatch::Ffunction(signature, _, _) = match_box(arena, ff) else {
        return Err(PropagateError::UnsupportedBox {
            node: ff,
            kind: "ffunction",
        });
    };
    let signature_len = list_length(arena, signature).ok_or(PropagateError::UnsupportedBox {
        node: signature,
        kind: "ffunction-signature",
    })?;
    signature_len
        .checked_sub(2)
        .ok_or(PropagateError::UnsupportedBox {
            node: signature,
            kind: "ffunction-signature",
        })
}

/// Fallible `usize -> i32` conversion used for stable signal-index nodes.
pub(crate) fn i32_from_usize(value: usize, field: &'static str) -> Result<i32, PropagateError> {
    i32::try_from(value).map_err(|_| PropagateError::IntegerTooLarge { field, value })
}

/// Seeds recursive feedback inputs with `delay1(proj(i, DEBRUIJNREF(1)))`.
pub(crate) fn make_mem_sig_proj_list(
    arena: &mut TreeArena,
    n: usize,
) -> Result<Vec<SigId>, PropagateError> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let idx = i32_from_usize(i, "rec projection seed index")?;
        let rg = debruijn_ref(arena, 1);
        let mut b = SigBuilder::new(arena);
        let proj = b.proj(idx, rg);
        out.push(b.delay1(proj));
    }
    Ok(out)
}

/// Local memoization reused across one propagation traversal.
///
/// The post-`eval/a2sb` signal trees are DAGs, so recursive helpers that walk
/// De Bruijn wrappers can revisit the same subtree many times from different
/// composition paths. Caching by `(TreeId, threshold)` and `TreeId` keeps the
/// operational lowering stable while avoiding repeated full-subtree rebuilds.
pub(crate) struct PropagateMemo {
    pub(crate) liftn: AHashMap<(TreeId, i64), TreeId>,
    pub(crate) aperture: AHashMap<TreeId, i64>,
}

impl Default for PropagateMemo {
    fn default() -> Self {
        Self {
            liftn: AHashMap::new(),
            aperture: AHashMap::new(),
        }
    }
}

/// Internal mutable context threaded through one propagation traversal.
///
/// This keeps analysis cache ownership (`ArityCache`) separate from local
/// traversal memoization (`PropagateMemo`) while avoiding wide internal helper
/// signatures.
pub(crate) struct PropagateContext<'a> {
    pub(crate) cache: &'a mut ArityCache,
    pub(crate) control_ids: &'a ControlIds,
    pub(crate) slot_env: &'a mut SlotEnv,
    pub(crate) memo: &'a mut PropagateMemo,
    /// Side table of clock-domain instances allocated by clocked wrappers
    /// during this propagation run (roadmap P0.2).
    pub(crate) clock_domains: &'a mut ClockDomainTable,
    /// Current clock environment as an in-graph value: `nil` at the top-level
    /// rate, otherwise the opaque `SIGCLOCKENV` token of `clock_domain`.
    pub(crate) clock_env: TreeId,
    /// Current clock-domain id (`None` at the top-level rate). Mirrors
    /// `clock_env`; kept separately so allocation can record the parent
    /// without re-decoding the token.
    pub(crate) clock_domain: Option<ClockDomainId>,
    /// When `true`, `ForwardAD` nodes act as transparent wrappers (no signal
    /// expansion).  Set while propagating Rec branches so that FAD expansion
    /// is deferred until after the recursive group is fully built.
    pub(crate) suppress_fad: bool,
    /// Seeds collected from `ForwardAD` nodes suppressed during Rec branch
    /// propagation. Drained by the Rec arm after the recursive group is built.
    pub(crate) pending_fad_seeds: Vec<SigId>,
    /// Accumulated UI group path at the current propagation position.
    /// Mirrors the `current_groups` stack maintained during UI collection so
    /// that widget control-id lookups use the same context-sensitive key.
    pub(crate) current_groups: Vec<UiGroupPathSegment>,
}

/// Lifts De Bruijn references of input signals by one recursion level.
pub(crate) fn lift_signals(
    arena: &mut TreeArena,
    inputs: &[SigId],
    memo: &mut PropagateMemo,
) -> Vec<SigId> {
    let mut out = Vec::with_capacity(inputs.len());
    for sig in inputs.iter().copied() {
        out.push(liftn(arena, sig, 1, memo));
    }
    out
}

/// Builds one recursive signal group wrapper (`DEBRUIJNREC(body)`).
pub(crate) fn debruijn_rec(arena: &mut TreeArena, body: TreeId) -> TreeId {
    intern_tag(arena, DEBRUIJNREC_TAG, &[body])
}

/// Builds one De Bruijn reference node (`DEBRUIJNREF(level)`).
pub(crate) fn debruijn_ref(arena: &mut TreeArena, level: i64) -> TreeId {
    let lvl = arena.int(level);
    intern_tag(arena, DEBRUIJNREF_TAG, &[lvl])
}

/// Recursively lifts De Bruijn reference levels starting at `threshold`.
pub(crate) fn liftn(
    arena: &mut TreeArena,
    root: TreeId,
    threshold: i64,
    memo: &mut PropagateMemo,
) -> TreeId {
    let key = (root, threshold);
    if let Some(lifted) = memo.liftn.get(&key).copied() {
        return lifted;
    }

    if let Some(level) = debruijn_ref_level(arena, root) {
        let lifted = if level < threshold {
            root
        } else {
            debruijn_ref(arena, level + 1)
        };
        memo.liftn.insert(key, lifted);
        return lifted;
    }

    if let Some(body) = debruijn_body(arena, root) {
        let lifted_body = liftn(arena, body, threshold + 1, memo);
        let lifted = debruijn_rec(arena, lifted_body);
        memo.liftn.insert(key, lifted);
        return lifted;
    }

    let Some(node) = arena.node(root).cloned() else {
        memo.liftn.insert(key, root);
        return root;
    };
    if node.children.is_empty() {
        memo.liftn.insert(key, root);
        return root;
    }

    let original_children = node.children.as_slice();
    let mut rebuilt = Vec::with_capacity(original_children.len());
    let mut changed = false;
    for child in original_children.iter().copied() {
        let lifted = liftn(arena, child, threshold, memo);
        if lifted != child {
            changed = true;
        }
        rebuilt.push(lifted);
    }
    let lifted = if changed {
        arena.intern(node.kind, &rebuilt)
    } else {
        root
    };
    memo.liftn.insert(key, lifted);
    lifted
}

// Aperture computation is delegated to `tlib::de_bruijn_aperture_with_memo`.
// The `PropagateMemo::aperture` cache is passed through so that aperture
// results are amortized across the full propagation traversal.

/// Returns De Bruijn level for a reference node, if `root` is `DEBRUIJNREF`.
pub(crate) fn debruijn_ref_level(arena: &TreeArena, root: TreeId) -> Option<i64> {
    let (tag, children) = tag_and_children(arena, root)?;
    if tag != DEBRUIJNREF_TAG {
        return None;
    }
    let [level_node] = children else {
        return None;
    };
    tree_to_int(arena, *level_node)
}

/// Returns recursive group body when `root` is a `DEBRUIJNREC` node.
pub(crate) fn debruijn_body(arena: &TreeArena, root: TreeId) -> Option<TreeId> {
    let (tag, children) = tag_and_children(arena, root)?;
    if tag != DEBRUIJNREC_TAG {
        return None;
    }
    let [body] = children else {
        return None;
    };
    Some(*body)
}

/// Helper to decode `(tag_name, children)` from one tagged node.
fn tag_and_children(arena: &TreeArena, root: TreeId) -> Option<(&str, &[TreeId])> {
    let node = arena.node(root)?;
    let NodeKind::Tag(tag_id) = &node.kind else {
        return None;
    };
    let tag = arena.tag_name(*tag_id)?;
    Some((tag, node.children.as_slice()))
}

/// Interns one tag node with children in the arena.
fn intern_tag(arena: &mut TreeArena, tag: &str, children: &[TreeId]) -> TreeId {
    let tag_id = arena.intern_tag(tag);
    arena.intern(NodeKind::Tag(tag_id), children)
}
