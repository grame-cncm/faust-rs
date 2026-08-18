//! `math_ext` instruction family of the FBC to C++ generator.
//!
//! Extended math (libm-style unary and binary), all addressing modes
//!
//! Split out of `fbc_to_cpp.rs` on 2026-08-18, where these arms were one
//! region of a single 1449-line `compile_instr`. The arms are moved verbatim.
//! The parameter list is this family's actual needs, so what a family does
//! not touch is visible from its signature.

use super::*;

impl BlockComp {
    /// Compiles one math_ext instruction into its native C++ equivalent.
    pub(super) fn compile_math_ext<R: FbcReal>(
        &mut self,
        out: &mut String,
        t: usize,
        instr: &FbcInstruction<R>,
    ) -> Result<(), FbcCppError> {
        use FbcOpcode::*;

        let o1 = instr.offset1;
        let o2 = instr.offset2;
        let iv = instr.int_value;
        let rv = instr.real_value;
        match instr.opcode {
            // ════════════════════════════════════════════════════════════
            // Extended unary math (stack)
            // ════════════════════════════════════════════════════════════
            Abs => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("std::abs({v})"));
            }
            Absf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::fabs({v})"));
            }
            Acosf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::acos({v})"));
            }
            Acoshf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::acosh({v})"));
            }
            Asinf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::asin({v})"));
            }
            Asinhf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::asinh({v})"));
            }
            Atanf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::atan({v})"));
            }
            Atanhf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::atanh({v})"));
            }
            Ceilf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::ceil({v})"));
            }
            Cosf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::cos({v})"));
            }
            Coshf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::cosh({v})"));
            }
            Expf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::exp({v})"));
            }
            Floorf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::floor({v})"));
            }
            Logf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::log({v})"));
            }
            Log10f => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::log10({v})"));
            }
            Rintf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::rint({v})"));
            }
            Roundf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::round({v})"));
            }
            Sinf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::sin({v})"));
            }
            Sinhf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::sinh({v})"));
            }
            Sqrtf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::sqrt({v})"));
            }
            Tanf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::tan({v})"));
            }
            Tanhf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::tanh({v})"));
            }
            Isnanf => {
                let v = self.pop_r();
                self.push_i(out, t, &format!("std::isnan({v})"));
            }
            Isinff => {
                let v = self.pop_r();
                self.push_i(out, t, &format!("std::isinf({v})"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended unary math (heap → stack)
            // ════════════════════════════════════════════════════════════
            AbsHeap => {
                self.push_i(out, t, &format!("std::abs(iVec[{o1}])"));
            }
            AbsfHeap => {
                self.push_r(out, t, &format!("std::fabs(fVec[{o1}])"));
            }
            AcosfHeap => {
                self.push_r(out, t, &format!("std::acos(fVec[{o1}])"));
            }
            AcoshfHeap => {
                self.push_r(out, t, &format!("std::acosh(fVec[{o1}])"));
            }
            AsinfHeap => {
                self.push_r(out, t, &format!("std::asin(fVec[{o1}])"));
            }
            AsinhfHeap => {
                self.push_r(out, t, &format!("std::asinh(fVec[{o1}])"));
            }
            AtanfHeap => {
                self.push_r(out, t, &format!("std::atan(fVec[{o1}])"));
            }
            AtanhfHeap => {
                self.push_r(out, t, &format!("std::atanh(fVec[{o1}])"));
            }
            CeilfHeap => {
                self.push_r(out, t, &format!("std::ceil(fVec[{o1}])"));
            }
            CosfHeap => {
                self.push_r(out, t, &format!("std::cos(fVec[{o1}])"));
            }
            CoshfHeap => {
                self.push_r(out, t, &format!("std::cosh(fVec[{o1}])"));
            }
            ExpfHeap => {
                self.push_r(out, t, &format!("std::exp(fVec[{o1}])"));
            }
            FloorfHeap => {
                self.push_r(out, t, &format!("std::floor(fVec[{o1}])"));
            }
            LogfHeap => {
                self.push_r(out, t, &format!("std::log(fVec[{o1}])"));
            }
            Log10fHeap => {
                self.push_r(out, t, &format!("std::log10(fVec[{o1}])"));
            }
            RintfHeap => {
                self.push_r(out, t, &format!("std::rint(fVec[{o1}])"));
            }
            RoundfHeap => {
                self.push_r(out, t, &format!("std::round(fVec[{o1}])"));
            }
            SinfHeap => {
                self.push_r(out, t, &format!("std::sin(fVec[{o1}])"));
            }
            SinhfHeap => {
                self.push_r(out, t, &format!("std::sinh(fVec[{o1}])"));
            }
            SqrtfHeap => {
                self.push_r(out, t, &format!("std::sqrt(fVec[{o1}])"));
            }
            TanfHeap => {
                self.push_r(out, t, &format!("std::tan(fVec[{o1}])"));
            }
            TanhfHeap => {
                self.push_r(out, t, &format!("std::tanh(fVec[{o1}])"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math (stack OP stack → push1)
            // ════════════════════════════════════════════════════════════
            Atan2f => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::atan2({v1}, {v2})"));
            }
            Fmodf => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::fmod({v1}, {v2})"));
            }
            Powf => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::pow({v1}, {v2})"));
            }
            Max => {
                let v1 = self.pop_i();
                let v2 = self.pop_i();
                self.push_i(out, t, &format!("std::max({v1}, {v2})"));
            }
            Maxf => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::max({v1}, {v2})"));
            }
            Min => {
                let v1 = self.pop_i();
                let v2 = self.pop_i();
                self.push_i(out, t, &format!("std::min({v1}, {v2})"));
            }
            Minf => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::min({v1}, {v2})"));
            }
            Copysignf => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::copysign({v1}, {v2})"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math (heap OP heap → push1)
            // ════════════════════════════════════════════════════════════
            Atan2fHeap => {
                self.push_r(out, t, &format!("std::atan2(fVec[{o1}], fVec[{o2}])"));
            }
            FmodfHeap => {
                self.push_r(out, t, &format!("std::fmod(fVec[{o1}], fVec[{o2}])"));
            }
            PowfHeap => {
                self.push_r(out, t, &format!("std::pow(fVec[{o1}], fVec[{o2}])"));
            }
            MaxHeap => {
                self.push_i(out, t, &format!("std::max(iVec[{o1}], iVec[{o2}])"));
            }
            MaxfHeap => {
                self.push_r(out, t, &format!("std::max(fVec[{o1}], fVec[{o2}])"));
            }
            MinHeap => {
                self.push_i(out, t, &format!("std::min(iVec[{o1}], iVec[{o2}])"));
            }
            MinfHeap => {
                self.push_r(out, t, &format!("std::min(fVec[{o1}], fVec[{o2}])"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math (heap OP stack → push1)
            // ════════════════════════════════════════════════════════════
            Atan2fStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::atan2(fVec[{o1}], {v})"));
            }
            FmodfStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::fmod(fVec[{o1}], {v})"));
            }
            PowfStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::pow(fVec[{o1}], {v})"));
            }
            MaxStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("std::max(iVec[{o1}], {v})"));
            }
            MaxfStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::max(fVec[{o1}], {v})"));
            }
            MinStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("std::min(iVec[{o1}], {v})"));
            }
            MinfStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::min(fVec[{o1}], {v})"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math (value OP stack → push1)
            // ════════════════════════════════════════════════════════════
            Atan2fStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::atan2({v}, {lit})"));
            }
            FmodfStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::fmod({v}, {lit})"));
            }
            PowfStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::pow({v}, {lit})"));
            }
            MaxStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("std::max({v}, {iv})"));
            }
            MaxfStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::max({v}, {lit})"));
            }
            MinStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("std::min({v}, {iv})"));
            }
            MinfStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::min({v}, {lit})"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math (value OP heap → push1)
            // ════════════════════════════════════════════════════════════
            Atan2fValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::atan2(fVec[{o1}], {lit})"));
            }
            FmodfValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::fmod(fVec[{o1}], {lit})"));
            }
            PowfValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::pow(fVec[{o1}], {lit})"));
            }
            MaxValue => {
                self.push_i(out, t, &format!("std::max(iVec[{o1}], {iv})"));
            }
            MaxfValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::max(fVec[{o1}], {lit})"));
            }
            MinValue => {
                self.push_i(out, t, &format!("std::min(iVec[{o1}], {iv})"));
            }
            MinfValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::min(fVec[{o1}], {lit})"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math: value OP heap — non-commutative inverted
            // ════════════════════════════════════════════════════════════
            Atan2fValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::atan2({lit}, fVec[{o1}])"));
            }
            FmodfValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::fmod({lit}, fVec[{o1}])"));
            }
            PowfValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::pow({lit}, fVec[{o1}])"));
            }

            other => unreachable!("math_ext dispatch received {other:?}"),
        }

        Ok(())
    }
}
