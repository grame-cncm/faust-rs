//! `BoxBuilder` — canonical construction API for box nodes.

use tlib::TreeArena;

use crate::BoxId;
use crate::internals::*;

/// Canonical builder API for constructing box nodes.
///
/// This is the preferred Rust API for new code.
pub struct BoxBuilder<'a> {
    arena: &'a mut TreeArena,
}

impl<'a> BoxBuilder<'a> {
    fn debug_assert_node_exists(&self, kind: &str, id: BoxId) {
        debug_assert!(
            self.arena.node(id).is_some(),
            "{kind} expects child node {} to exist in the bound TreeArena",
            id.as_u32()
        );
    }

    #[must_use]
    /// Creates a `BoxBuilder` bound to one mutable `TreeArena`.
    pub fn new(arena: &'a mut TreeArena) -> Self {
        Self { arena }
    }

    #[must_use]
    /// Builds one box node for `ident` and returns its `BoxId`.
    pub fn ident(&mut self, name: &str) -> BoxId {
        node_ident(self.arena, name)
    }

    #[must_use]
    /// Builds one box node for `int` and returns its `BoxId`.
    pub fn int(&mut self, value: i32) -> BoxId {
        node_int(self.arena, value)
    }

    #[must_use]
    /// Builds one box node for `real` and returns its `BoxId`.
    pub fn real(&mut self, value: f64) -> BoxId {
        node_real(self.arena, value)
    }

    #[must_use]
    /// Builds one box node for `wire` and returns its `BoxId`.
    pub fn wire(&mut self) -> BoxId {
        node_wire(self.arena)
    }

    #[must_use]
    /// Builds one box node for `cut` and returns its `BoxId`.
    pub fn cut(&mut self) -> BoxId {
        node_cut(self.arena)
    }

    #[must_use]
    /// Builds one box node for `seq` and returns its `BoxId`.
    pub fn seq(&mut self, left: BoxId, right: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxSeq", left);
        self.debug_assert_node_exists("boxSeq", right);
        node_seq(self.arena, left, right)
    }

    #[must_use]
    /// Builds one box node for `par` and returns its `BoxId`.
    pub fn par(&mut self, left: BoxId, right: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxPar", left);
        self.debug_assert_node_exists("boxPar", right);
        node_par(self.arena, left, right)
    }

    #[must_use]
    /// Builds one box node for `rec` and returns its `BoxId`.
    pub fn rec(&mut self, left: BoxId, right: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxRec", left);
        self.debug_assert_node_exists("boxRec", right);
        node_rec(self.arena, left, right)
    }

    #[must_use]
    /// Builds one box node for `split` and returns its `BoxId`.
    pub fn split(&mut self, left: BoxId, right: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxSplit", left);
        self.debug_assert_node_exists("boxSplit", right);
        node_split(self.arena, left, right)
    }

    #[must_use]
    /// Builds one box node for `merge` and returns its `BoxId`.
    pub fn merge(&mut self, left: BoxId, right: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxMerge", left);
        self.debug_assert_node_exists("boxMerge", right);
        node_merge(self.arena, left, right)
    }

    #[must_use]
    /// Builds one box node for `appl` and returns its `BoxId`.
    pub fn appl(&mut self, fun: BoxId, arglist: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxAppl", fun);
        self.debug_assert_node_exists("boxAppl", arglist);
        node_appl(self.arena, fun, arglist)
    }

    #[must_use]
    /// Builds one box node for `access` and returns its `BoxId`.
    pub fn access(&mut self, expr: BoxId, ident: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxAccess", expr);
        self.debug_assert_node_exists("boxAccess", ident);
        node_access(self.arena, expr, ident)
    }

    #[must_use]
    /// Builds one box node for `add` and returns its `BoxId`.
    pub fn add(&mut self) -> BoxId {
        node_add(self.arena)
    }

    #[must_use]
    /// Builds one box node for `sub` and returns its `BoxId`.
    pub fn sub(&mut self) -> BoxId {
        node_sub(self.arena)
    }

    #[must_use]
    /// Builds one box node for `mul` and returns its `BoxId`.
    pub fn mul(&mut self) -> BoxId {
        node_mul(self.arena)
    }

    #[must_use]
    /// Builds one box node for `div` and returns its `BoxId`.
    pub fn div(&mut self) -> BoxId {
        node_div(self.arena)
    }

    #[must_use]
    /// Builds one box node for `rem` and returns its `BoxId`.
    pub fn rem(&mut self) -> BoxId {
        node_rem(self.arena)
    }

    #[must_use]
    /// Builds one box node for `and` and returns its `BoxId`.
    pub fn and(&mut self) -> BoxId {
        node_and(self.arena)
    }

    #[must_use]
    /// Builds one box node for `or` and returns its `BoxId`.
    pub fn or(&mut self) -> BoxId {
        node_or(self.arena)
    }

    #[must_use]
    /// Builds one box node for `xor` and returns its `BoxId`.
    pub fn xor(&mut self) -> BoxId {
        node_xor(self.arena)
    }

    #[must_use]
    /// Builds one box node for `lsh` and returns its `BoxId`.
    pub fn lsh(&mut self) -> BoxId {
        node_lsh(self.arena)
    }

    #[must_use]
    /// Builds one box node for `rsh` and returns its `BoxId`.
    pub fn rsh(&mut self) -> BoxId {
        node_rsh(self.arena)
    }

    #[must_use]
    /// Builds one box node for logical right shift and returns its `BoxId`.
    pub fn lrsh(&mut self) -> BoxId {
        node_lrsh(self.arena)
    }

    #[must_use]
    /// Builds one box node for `lt` and returns its `BoxId`.
    pub fn lt(&mut self) -> BoxId {
        node_lt(self.arena)
    }

    #[must_use]
    /// Builds one box node for `le` and returns its `BoxId`.
    pub fn le(&mut self) -> BoxId {
        node_le(self.arena)
    }

    #[must_use]
    /// Builds one box node for `gt` and returns its `BoxId`.
    pub fn gt(&mut self) -> BoxId {
        node_gt(self.arena)
    }

    #[must_use]
    /// Builds one box node for `ge` and returns its `BoxId`.
    pub fn ge(&mut self) -> BoxId {
        node_ge(self.arena)
    }

    #[must_use]
    /// Builds one box node for `eq` and returns its `BoxId`.
    pub fn eq(&mut self) -> BoxId {
        node_eq(self.arena)
    }

    #[must_use]
    /// Builds one box node for `ne` and returns its `BoxId`.
    pub fn ne(&mut self) -> BoxId {
        node_ne(self.arena)
    }

    #[must_use]
    /// Builds one box node for `pow` and returns its `BoxId`.
    pub fn pow(&mut self) -> BoxId {
        node_pow(self.arena)
    }

    #[must_use]
    /// Builds one box node for `acos` and returns its `BoxId`.
    pub fn acos(&mut self) -> BoxId {
        node_acos(self.arena)
    }

    #[must_use]
    /// Builds one box node for `asin` and returns its `BoxId`.
    pub fn asin(&mut self) -> BoxId {
        node_asin(self.arena)
    }

    #[must_use]
    /// Builds one box node for `atan` and returns its `BoxId`.
    pub fn atan(&mut self) -> BoxId {
        node_atan(self.arena)
    }

    #[must_use]
    /// Builds one box node for `atan2` and returns its `BoxId`.
    pub fn atan2(&mut self) -> BoxId {
        node_atan2(self.arena)
    }

    #[must_use]
    /// Builds one box node for `cos` and returns its `BoxId`.
    pub fn cos(&mut self) -> BoxId {
        node_cos(self.arena)
    }

    #[must_use]
    /// Builds one box node for `sin` and returns its `BoxId`.
    pub fn sin(&mut self) -> BoxId {
        node_sin(self.arena)
    }

    #[must_use]
    /// Builds one box node for `tan` and returns its `BoxId`.
    pub fn tan(&mut self) -> BoxId {
        node_tan(self.arena)
    }

    #[must_use]
    /// Builds one box node for `exp` and returns its `BoxId`.
    pub fn exp(&mut self) -> BoxId {
        node_exp(self.arena)
    }

    #[must_use]
    /// Builds one box node for `exp10` and returns its `BoxId`.
    pub fn exp10(&mut self) -> BoxId {
        node_exp10(self.arena)
    }

    #[must_use]
    /// Builds one box node for `log` and returns its `BoxId`.
    pub fn log(&mut self) -> BoxId {
        node_log(self.arena)
    }

    #[must_use]
    /// Builds one box node for `log10` and returns its `BoxId`.
    pub fn log10(&mut self) -> BoxId {
        node_log10(self.arena)
    }

    #[must_use]
    /// Builds one box node for `sqrt` and returns its `BoxId`.
    pub fn sqrt(&mut self) -> BoxId {
        node_sqrt(self.arena)
    }

    #[must_use]
    /// Builds one box node for `abs` and returns its `BoxId`.
    pub fn abs(&mut self) -> BoxId {
        node_abs(self.arena)
    }

    #[must_use]
    /// Builds one box node for `fmod` and returns its `BoxId`.
    pub fn fmod(&mut self) -> BoxId {
        node_fmod(self.arena)
    }

    #[must_use]
    /// Builds one box node for `remainder` and returns its `BoxId`.
    pub fn remainder(&mut self) -> BoxId {
        node_remainder(self.arena)
    }

    #[must_use]
    /// Builds one box node for `floor` and returns its `BoxId`.
    pub fn floor(&mut self) -> BoxId {
        node_floor(self.arena)
    }

    #[must_use]
    /// Builds one box node for `ceil` and returns its `BoxId`.
    pub fn ceil(&mut self) -> BoxId {
        node_ceil(self.arena)
    }

    #[must_use]
    /// Builds one box node for `rint` and returns its `BoxId`.
    pub fn rint(&mut self) -> BoxId {
        node_rint(self.arena)
    }

    #[must_use]
    /// Builds one box node for `round` and returns its `BoxId`.
    pub fn round(&mut self) -> BoxId {
        node_round(self.arena)
    }

    #[must_use]
    /// Builds one box node for `delay` and returns its `BoxId`.
    pub fn delay(&mut self) -> BoxId {
        node_delay(self.arena)
    }

    #[must_use]
    /// Builds one box node for `delay1` and returns its `BoxId`.
    pub fn delay1(&mut self) -> BoxId {
        node_delay1(self.arena)
    }

    #[must_use]
    /// Builds one box node for `min` and returns its `BoxId`.
    pub fn min(&mut self) -> BoxId {
        node_min(self.arena)
    }

    #[must_use]
    /// Builds one box node for `max` and returns its `BoxId`.
    pub fn max(&mut self) -> BoxId {
        node_max(self.arena)
    }

    #[must_use]
    /// Builds one box node for `prefix` and returns its `BoxId`.
    pub fn prefix(&mut self) -> BoxId {
        node_prefix(self.arena)
    }

    #[must_use]
    /// Builds one box node for `int_cast` and returns its `BoxId`.
    pub fn int_cast(&mut self) -> BoxId {
        node_int_cast(self.arena)
    }

    #[must_use]
    /// Builds one box node for `float_cast` and returns its `BoxId`.
    pub fn float_cast(&mut self) -> BoxId {
        node_float_cast(self.arena)
    }

    #[must_use]
    /// Builds one box node for `read_only_table` and returns its `BoxId`.
    pub fn read_only_table(&mut self) -> BoxId {
        node_read_only_table(self.arena)
    }

    #[must_use]
    /// Builds one box node for `write_read_table` and returns its `BoxId`.
    pub fn write_read_table(&mut self) -> BoxId {
        node_write_read_table(self.arena)
    }

    #[must_use]
    /// Builds one box node for `select2` and returns its `BoxId`.
    pub fn select2(&mut self) -> BoxId {
        node_select2(self.arena)
    }

    #[must_use]
    /// Builds one box node for `select3` and returns its `BoxId`.
    pub fn select3(&mut self) -> BoxId {
        node_select3(self.arena)
    }

    #[must_use]
    /// Builds one box node for `assert_bounds` and returns its `BoxId`.
    pub fn assert_bounds(&mut self) -> BoxId {
        node_assert_bounds(self.arena)
    }

    #[must_use]
    /// Builds one box node for `lowest` and returns its `BoxId`.
    pub fn lowest(&mut self) -> BoxId {
        node_lowest(self.arena)
    }

    #[must_use]
    /// Builds one box node for `highest` and returns its `BoxId`.
    pub fn highest(&mut self) -> BoxId {
        node_highest(self.arena)
    }

    #[must_use]
    /// Builds one box node for `attach` and returns its `BoxId`.
    pub fn attach(&mut self) -> BoxId {
        node_attach(self.arena)
    }

    #[must_use]
    /// Builds one box node for `enable` and returns its `BoxId`.
    pub fn enable(&mut self) -> BoxId {
        node_enable(self.arena)
    }

    #[must_use]
    /// Builds one box node for `control` and returns its `BoxId`.
    pub fn control(&mut self) -> BoxId {
        node_control(self.arena)
    }

    #[must_use]
    /// Builds one box node for `slot` and returns its `BoxId`.
    pub fn slot(&mut self, id: i32) -> BoxId {
        node_slot(self.arena, id)
    }

    #[must_use]
    /// Builds one box node for `symbolic` and returns its `BoxId`.
    pub fn symbolic(&mut self, slot: BoxId, body: BoxId) -> BoxId {
        node_symbolic(self.arena, slot, body)
    }

    #[must_use]
    /// Builds one box node for `ipar` and returns its `BoxId`.
    pub fn ipar(&mut self, index: BoxId, count: BoxId, body: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxIPar", index);
        self.debug_assert_node_exists("boxIPar", count);
        self.debug_assert_node_exists("boxIPar", body);
        node_ipar(self.arena, index, count, body)
    }

    #[must_use]
    /// Builds one box node for `iseq` and returns its `BoxId`.
    pub fn iseq(&mut self, index: BoxId, count: BoxId, body: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxISeq", index);
        self.debug_assert_node_exists("boxISeq", count);
        self.debug_assert_node_exists("boxISeq", body);
        node_iseq(self.arena, index, count, body)
    }

    #[must_use]
    /// Builds one box node for `isum` and returns its `BoxId`.
    pub fn isum(&mut self, index: BoxId, count: BoxId, body: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxISum", index);
        self.debug_assert_node_exists("boxISum", count);
        self.debug_assert_node_exists("boxISum", body);
        node_isum(self.arena, index, count, body)
    }

    #[must_use]
    /// Builds one box node for `iprod` and returns its `BoxId`.
    pub fn iprod(&mut self, index: BoxId, count: BoxId, body: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxIProd", index);
        self.debug_assert_node_exists("boxIProd", count);
        self.debug_assert_node_exists("boxIProd", body);
        node_iprod(self.arena, index, count, body)
    }

    #[must_use]
    /// Builds one box node for `with_local_def` and returns its `BoxId`.
    pub fn with_local_def(&mut self, body: BoxId, ldef: BoxId) -> BoxId {
        node_with_local_def(self.arena, body, ldef)
    }

    #[must_use]
    /// Builds one box node for `modif_local_def` and returns its `BoxId`.
    pub fn modif_local_def(&mut self, body: BoxId, ldef: BoxId) -> BoxId {
        node_modif_local_def(self.arena, body, ldef)
    }

    #[must_use]
    /// Builds one box node for `with_rec_def` and returns its `BoxId`.
    pub fn with_rec_def(&mut self, body: BoxId, ldef: BoxId, ldef2: BoxId) -> BoxId {
        node_with_rec_def(self.arena, body, ldef, ldef2)
    }

    #[must_use]
    /// Builds one box node for `metadata` and returns its `BoxId`.
    ///
    /// `mdlist` keeps the C++ pair encoding used by the parser/evaluator stack:
    /// `cons(key, value)`.
    pub fn metadata(&mut self, expr: BoxId, mdlist: BoxId) -> BoxId {
        node_metadata(self.arena, expr, mdlist)
    }

    #[must_use]
    /// Builds one box node for `environment` and returns its `BoxId`.
    pub fn environment(&mut self) -> BoxId {
        node_environment(self.arena)
    }

    #[must_use]
    /// Builds one box node for `component` and returns its `BoxId`.
    pub fn component(&mut self, filename: BoxId) -> BoxId {
        node_component(self.arena, filename)
    }

    #[must_use]
    /// Builds one box node for `library` and returns its `BoxId`.
    pub fn library(&mut self, filename: BoxId) -> BoxId {
        node_library(self.arena, filename)
    }

    #[must_use]
    /// Builds one parser/import node for `importFile` and returns its `BoxId`.
    ///
    /// Source provenance (C++):
    /// - `compiler/boxes/boxes.cpp`
    /// - `importFile(Tree filename)`
    pub fn import_file(&mut self, filename: BoxId) -> BoxId {
        node_import_file(self.arena, filename)
    }

    #[must_use]
    /// Builds one box node for `waveform` and returns its `BoxId`.
    pub fn waveform(&mut self, values: &[BoxId]) -> BoxId {
        for &value in values {
            self.debug_assert_node_exists("boxWaveform", value);
        }
        node_waveform(self.arena, values)
    }

    #[must_use]
    /// Builds one box node for `route` and returns its `BoxId`.
    pub fn route(&mut self, n: BoxId, m: BoxId, route_spec: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxRoute", n);
        self.debug_assert_node_exists("boxRoute", m);
        self.debug_assert_node_exists("boxRoute", route_spec);
        node_route(self.arena, n, m, route_spec)
    }

    #[must_use]
    /// Builds one box node for `ffunction` and returns its `BoxId`.
    pub fn ffunction(&mut self, signature: BoxId, incfile: BoxId, libfile: BoxId) -> BoxId {
        ffunction(self.arena, signature, incfile, libfile)
    }

    #[must_use]
    /// Builds one box node for `ffun` and returns its `BoxId`.
    pub fn ffun(&mut self, ff: BoxId) -> BoxId {
        node_ffun(self.arena, ff)
    }

    #[must_use]
    /// Builds one box node for `fconst` and returns its `BoxId`.
    pub fn fconst(&mut self, ty: BoxId, name: BoxId, file: BoxId) -> BoxId {
        node_fconst(self.arena, ty, name, file)
    }

    #[must_use]
    /// Builds one box node for `fvar` and returns its `BoxId`.
    pub fn fvar(&mut self, ty: BoxId, name: BoxId, file: BoxId) -> BoxId {
        node_fvar(self.arena, ty, name, file)
    }

    #[must_use]
    /// Builds one box node for `case` and returns its `BoxId`.
    pub fn case(&mut self, rules: BoxId) -> BoxId {
        node_case(self.arena, rules)
    }

    #[must_use]
    /// Builds a `boxPatternMatcher` node — a handle to a partially-applied PM in
    /// the evaluator's side-table.
    pub fn pattern_matcher(&mut self, key: BoxId) -> BoxId {
        node_pattern_matcher(self.arena, key)
    }

    /// Builds a `boxClosure` node — a handle to a closure in the evaluator's
    /// side-table.
    pub fn closure_node(&mut self, key: BoxId) -> BoxId {
        node_closure(self.arena, key)
    }

    #[must_use]
    /// Builds one box node for `pattern_var` and returns its `BoxId`.
    pub fn pattern_var(&mut self, ident: BoxId) -> BoxId {
        node_pattern_var(self.arena, ident)
    }

    #[must_use]
    /// Builds one box node for `abstr` and returns its `BoxId`.
    pub fn abstr(&mut self, arg: BoxId, body: BoxId) -> BoxId {
        node_abstr(self.arena, arg, body)
    }

    #[must_use]
    /// Builds one box node for `modulation` and returns its `BoxId`.
    pub fn modulation(&mut self, arg: BoxId, body: BoxId) -> BoxId {
        node_modulation(self.arena, arg, body)
    }

    #[must_use]
    /// Builds one box node for `build_abstr` and returns its `BoxId`.
    pub fn build_abstr(&mut self, args: BoxId, body: BoxId) -> BoxId {
        build_box_abstr(self.arena, args, body)
    }

    #[must_use]
    /// Builds one box node for `build_modulation` and returns its `BoxId`.
    pub fn build_modulation(&mut self, args: BoxId, body: BoxId) -> BoxId {
        build_box_modulation(self.arena, args, body)
    }

    #[must_use]
    /// Builds one box node for `inputs` and returns its `BoxId`.
    pub fn inputs(&mut self, expr: BoxId) -> BoxId {
        node_inputs(self.arena, expr)
    }

    #[must_use]
    /// Builds one box node for `outputs` and returns its `BoxId`.
    pub fn outputs(&mut self, expr: BoxId) -> BoxId {
        node_outputs(self.arena, expr)
    }

    #[must_use]
    /// Builds `cinputs(expr)`, the control inputs of `expr` as a list of its
    /// widget boxes (faust-rs extension).
    pub fn cinputs(&mut self, expr: BoxId) -> BoxId {
        node_cinputs(self.arena, expr)
    }

    #[must_use]
    /// Builds `cinput(index, expr)`, the `index`-th control input of `expr` as
    /// the list `(widget, init, min, max, step)` (faust-rs extension).
    pub fn cinput(&mut self, index: BoxId, expr: BoxId) -> BoxId {
        node_cinput(self.arena, index, expr)
    }

    #[must_use]
    /// Builds `coutputs(expr)`, the bargraphs of `expr` as a list of its
    /// bargraph boxes (faust-rs extension).
    pub fn coutputs(&mut self, expr: BoxId) -> BoxId {
        node_coutputs(self.arena, expr)
    }

    #[must_use]
    /// Builds `coutput(index, expr)`, the `index`-th bargraph of `expr` as the
    /// list `(bargraph, min, max)` (faust-rs extension).
    pub fn coutput(&mut self, index: BoxId, expr: BoxId) -> BoxId {
        node_coutput(self.arena, index, expr)
    }

    #[must_use]
    /// Builds one box node for `fad(expr, seed)` and returns its `BoxId`.
    ///
    /// Source provenance (C++):
    /// - `compiler/boxes/boxes.cpp`
    /// - `boxForwardAD(Tree exp, Tree seed)`
    pub fn forward_ad(&mut self, expr: BoxId, seed: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxForwardAD expr", expr);
        self.debug_assert_node_exists("boxForwardAD seed", seed);
        node_forward_ad(self.arena, expr, seed)
    }

    #[must_use]
    /// Builds one box node for `rad(expr, seeds)` and returns its `BoxId`.
    ///
    /// Mirrors the explicit-seed shape of [`Self::forward_ad`]: `expr` is the
    /// expression bundle to differentiate, `seeds` is the block whose outputs
    /// are the independent variables. The two-child shape is preserved
    /// through eval and validated again in `propagate`.
    pub fn reverse_ad(&mut self, expr: BoxId, seeds: BoxId) -> BoxId {
        self.debug_assert_node_exists("boxReverseAD expr", expr);
        self.debug_assert_node_exists("boxReverseAD seeds", seeds);
        node_reverse_ad(self.arena, expr, seeds)
    }

    #[must_use]
    /// Builds one box node for `ondemand` and returns its `BoxId`.
    pub fn ondemand(&mut self, expr: BoxId) -> BoxId {
        node_ondemand(self.arena, expr)
    }

    #[must_use]
    /// Builds one box node for `upsampling` and returns its `BoxId`.
    pub fn upsampling(&mut self, expr: BoxId) -> BoxId {
        node_upsampling(self.arena, expr)
    }

    #[must_use]
    /// Builds one box node for `downsampling` and returns its `BoxId`.
    pub fn downsampling(&mut self, expr: BoxId) -> BoxId {
        node_downsampling(self.arena, expr)
    }

    #[must_use]
    /// Builds one box node for `button` and returns its `BoxId`.
    pub fn button(&mut self, label: BoxId) -> BoxId {
        node_button(self.arena, label)
    }

    #[must_use]
    /// Builds one box node for `checkbox` and returns its `BoxId`.
    pub fn checkbox(&mut self, label: BoxId) -> BoxId {
        node_checkbox(self.arena, label)
    }

    #[must_use]
    /// Builds one box node for `vslider` and returns its `BoxId`.
    pub fn vslider(
        &mut self,
        label: BoxId,
        cur: BoxId,
        min: BoxId,
        max: BoxId,
        step: BoxId,
    ) -> BoxId {
        node_vslider(self.arena, label, cur, min, max, step)
    }

    #[must_use]
    /// Builds one box node for `hslider` and returns its `BoxId`.
    pub fn hslider(
        &mut self,
        label: BoxId,
        cur: BoxId,
        min: BoxId,
        max: BoxId,
        step: BoxId,
    ) -> BoxId {
        node_hslider(self.arena, label, cur, min, max, step)
    }

    #[must_use]
    /// Builds one box node for `num_entry` and returns its `BoxId`.
    pub fn num_entry(
        &mut self,
        label: BoxId,
        cur: BoxId,
        min: BoxId,
        max: BoxId,
        step: BoxId,
    ) -> BoxId {
        node_num_entry(self.arena, label, cur, min, max, step)
    }

    #[must_use]
    /// Builds one box node for `vgroup` and returns its `BoxId`.
    pub fn vgroup(&mut self, label: BoxId, expr: BoxId) -> BoxId {
        node_vgroup(self.arena, label, expr)
    }

    #[must_use]
    /// Builds one box node for `hgroup` and returns its `BoxId`.
    pub fn hgroup(&mut self, label: BoxId, expr: BoxId) -> BoxId {
        node_hgroup(self.arena, label, expr)
    }

    #[must_use]
    /// Builds one box node for `tgroup` and returns its `BoxId`.
    pub fn tgroup(&mut self, label: BoxId, expr: BoxId) -> BoxId {
        node_tgroup(self.arena, label, expr)
    }

    #[must_use]
    /// Builds one box node for `vbargraph` and returns its `BoxId`.
    pub fn vbargraph(&mut self, label: BoxId, min: BoxId, max: BoxId) -> BoxId {
        node_vbargraph(self.arena, label, min, max)
    }

    #[must_use]
    /// Builds one box node for `hbargraph` and returns its `BoxId`.
    pub fn hbargraph(&mut self, label: BoxId, min: BoxId, max: BoxId) -> BoxId {
        node_hbargraph(self.arena, label, min, max)
    }

    #[must_use]
    /// Builds one box node for `soundfile` and returns its `BoxId`.
    pub fn soundfile(&mut self, label: BoxId, chan: BoxId) -> BoxId {
        node_soundfile(self.arena, label, chan)
    }
}
