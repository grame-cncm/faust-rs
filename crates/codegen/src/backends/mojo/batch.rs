//! The batch contract of `MojoOptions::batch` (GPU batch plan G3.5,
//! `porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md` §3.5,
//! §10): the host side of the struct (arena sizes, the state buffer, the
//! controls) and the voice in the instance lifecycle signatures.

use std::fmt::Write as _;

use super::{CodegenError, Emitter};

impl Emitter<'_> {
    /// The host side of the batch contract: per arena its elements per
    /// voice, the state buffer size for N voices (the arenas, each aligned to
    /// 8 bytes, in the order the constructor carves them), and the controls
    /// through the control table (design §6, D2).
    pub(super) fn emit_arena_sizes(
        &mut self,
        out: &mut String,
        layout: &fir::batch::VoiceLayout,
    ) -> Result<(), CodegenError> {
        let mut sizes = Vec::new();
        for arena in &layout.arenas {
            let name = self.fields.get(&arena.name)?;
            let elem = self.emit_type(&arena.elem_type)?;
            let _ = writeln!(out, "    @staticmethod");
            let _ = writeln!(out, "    def {name}_elements() -> Int:");
            let _ = writeln!(out, "        return {}\n", arena.elements);
            sizes.push(format!(
                "(Self.{name}_elements() * voices * size_of[{elem}]() + 7) // 8 * 8"
            ));
        }
        let _ = writeln!(out, "    @staticmethod");
        let _ = writeln!(out, "    def state_bytes(voices: Int) -> Int:");
        if sizes.is_empty() {
            let _ = writeln!(out, "        return 0\n");
        } else {
            let _ = writeln!(out, "        return {}\n", sizes.join(" + "));
        }

        let _ = writeln!(out, "    @staticmethod");
        let _ = writeln!(out, "    def num_controls() -> Int:");
        let _ = writeln!(out, "        return {}\n", layout.controls.len());
        let arena = match layout.controls.first().and_then(|c| layout.placement(c)) {
            Some(p) => Some(self.fields.get(&p.arena)?),
            None => None,
        };
        let table = self.fields.get(fir::batch::CONTROLS_TABLE)?;
        let _ = writeln!(out, "    @always_inline");
        let _ = writeln!(
            out,
            "    def set_control(mut dsp, var voice: S32, index: Int, value: FaustFloat) -> None:"
        );
        match &arena {
            Some(arena) => {
                let _ = writeln!(
                    out,
                    "        dsp.{arena}[unsafe_offset = dsp.{table}[index] * dsp.voices + voice] = value\n"
                );
            }
            None => {
                let _ = writeln!(out, "        pass\n");
            }
        }
        let _ = writeln!(out, "    @always_inline");
        let _ = writeln!(
            out,
            "    def get_control(imm dsp, var voice: S32, index: Int) -> FaustFloat:"
        );
        match &arena {
            Some(arena) => {
                let _ = writeln!(
                    out,
                    "        return dsp.{arena}[unsafe_offset = dsp.{table}[index] * dsp.voices + voice]\n"
                );
            }
            None => {
                let _ = writeln!(out, "        return FaustFloat(0.0)\n");
            }
        }
        Ok(())
    }

    /// `signature` with the voice argument after `dsp` in batch mode, for the
    /// instance lifecycle (`instance_*`, `init`, `compute`).
    pub(super) fn voiced(&self, signature: &str) -> String {
        if self.batch.is_none() {
            return signature.to_owned();
        }
        if let Some(rest) = signature.strip_suffix("(mut dsp) -> None:") {
            return format!("{rest}(mut dsp, var voice: S32) -> None:");
        }
        signature.replacen("(mut dsp, ", "(mut dsp, var voice: S32, ", 1)
    }

    /// `voice, ` in batch mode, for the calls between lifecycle methods.
    pub(super) fn voice_arg(&self) -> &'static str {
        if self.batch.is_some() { "voice, " } else { "" }
    }
}
