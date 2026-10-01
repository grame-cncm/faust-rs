//! Soundfile data structure for the interpreter backend.
//!
//! # Source provenance (C++)
//! - `Soundfile` struct in `architecture/faust/sound_player.h`
//! - Per-part metadata (`fLength`, `fSR`, `fOffset`) and channel buffers
//!   (`fBuffers[chan][sample_idx]`).
//!
//! In the Rust interpreter sample data is stored as `f64` regardless of the
//! DSP precision mode; the executor converts on the fly via `FbcReal::from_f64`.
//!
//! Runtime contract (C++ `architecture/faust/gui/Soundfile.h`): `num_channels
//! >= 1` and `buffers` holds the `num_channels` real channels, no more. A
//! channel requested beyond them is wrapped by the generated code
//! (`chan % fChannels`, see `transform`'s `emit_soundfile_buffer`), not here.

/// Runtime soundfile data: per-part metadata and interleaved channel buffers.
///
/// # C++ parity
/// - `Soundfile` in `architecture/faust/sound_player.h`
/// - `fLength[part]`, `fSR[part]`, `fOffset[part]`, `fBuffers[chan][idx]`
#[derive(Debug, Clone)]
pub struct Soundfile {
    /// Number of audio channels.
    pub num_channels: usize,
    /// Number of parts (independent clips within the soundfile).
    pub num_parts: usize,
    /// Sample count per part: `fLength[part]`.
    pub lengths: Vec<i32>,
    /// Sample rate per part in Hz: `fSR[part]`.
    pub sample_rates: Vec<i32>,
    /// Start offset of each part inside `buffers[chan]`: `fOffset[part]`.
    pub offsets: Vec<i32>,
    /// Per-channel sample data: `buffers[chan][sample_idx]`.
    pub buffers: Vec<Vec<f64>>,
}

impl Soundfile {
    /// Creates a default 1-part, 1-channel silence soundfile.
    ///
    /// This mirrors the C++ `defaultsound` fallback used when no audio file
    /// is provided or when soundfile loading is not yet supported:
    /// zero-length buffer at the standard 44100 Hz sample rate.
    #[must_use]
    pub fn default_silence() -> Self {
        Self {
            num_channels: 1,
            num_parts: 1,
            lengths: vec![0],
            sample_rates: vec![44100],
            offsets: vec![0],
            buffers: vec![vec![]],
        }
    }

    /// Creates an in-memory soundfile fixture equivalent to the C++ impulse
    /// tests' `TestMemoryReader`.
    ///
    /// # Source provenance (C++)
    /// - `tests/impulse-tests/archs/controlTools.h::TestMemoryReader`
    /// - `architecture/faust/gui/Soundfile.h::createSoundfile`
    ///
    /// The real resources are 2-channel, 4096-frame, 44100 Hz sinusoidal
    /// clips, `sin(part + chan + 2*pi*i/4096)`: each channel has its own
    /// phase, so a soundfile read with more outputs than it has channels shows
    /// the `chan % fChannels` wrap. Remaining soundfile parts are filled with
    /// the standard empty 1024-frame silent parts so part metadata matches the
    /// C++ `Soundfile`.
    #[must_use]
    pub fn impulse_test_memory_reader(num_real_parts: usize) -> Self {
        const SOUND_CHAN: usize = 2;
        const SOUND_LENGTH: usize = 4096;
        const SOUND_SR: i32 = 44100;
        const BUFFER_SIZE: usize = 1024;
        const MAX_SOUNDFILE_PARTS: usize = 256;

        let real_parts = num_real_parts.min(MAX_SOUNDFILE_PARTS);
        let mut lengths = Vec::with_capacity(MAX_SOUNDFILE_PARTS);
        let mut sample_rates = Vec::with_capacity(MAX_SOUNDFILE_PARTS);
        let mut offsets = Vec::with_capacity(MAX_SOUNDFILE_PARTS);
        let mut offset = 0usize;

        for _part in 0..real_parts {
            lengths.push(SOUND_LENGTH as i32);
            sample_rates.push(SOUND_SR);
            offsets.push(offset as i32);
            offset += SOUND_LENGTH;
        }
        for _part in real_parts..MAX_SOUNDFILE_PARTS {
            lengths.push(BUFFER_SIZE as i32);
            sample_rates.push(SOUND_SR);
            offsets.push(offset as i32);
            offset += BUFFER_SIZE;
        }

        let mut buffers = vec![vec![0.0; offset]; SOUND_CHAN];
        for (part, part_offset) in offsets.iter().copied().enumerate().take(real_parts) {
            let part_offset = part_offset as usize;
            for sample in 0..SOUND_LENGTH {
                for (chan, channel) in buffers.iter_mut().enumerate() {
                    channel[part_offset + sample] = (part as f64
                        + chan as f64
                        + (2.0 * std::f64::consts::PI * sample as f64 / SOUND_LENGTH as f64))
                        .sin();
                }
            }
        }

        Self {
            num_channels: SOUND_CHAN,
            num_parts: real_parts,
            lengths,
            sample_rates,
            offsets,
            buffers,
        }
    }

    /// Returns the sample at `buffers[chan][offsets[part] + idx]`.
    ///
    /// Out-of-bounds channel/part/sample accesses return `0.0` (silence). The
    /// generated code has already wrapped `chan` modulo `num_channels`, so an
    /// out-of-range channel only comes from malformed bytecode.
    #[must_use]
    pub fn read_sample(&self, chan: usize, part: usize, idx: i32) -> f64 {
        let offset = self.offsets.get(part).copied().unwrap_or(0) as usize;
        let sample_idx = offset.saturating_add(idx.max(0) as usize);
        self.buffers
            .get(chan)
            .and_then(|buf| buf.get(sample_idx))
            .copied()
            .unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Soundfile;

    #[test]
    fn read_sample_reads_real_channels_only() {
        let sf = Soundfile {
            num_channels: 2,
            num_parts: 1,
            lengths: vec![2],
            sample_rates: vec![44100],
            offsets: vec![0],
            buffers: vec![vec![0.25, 0.5], vec![0.75, 1.0]],
        };

        assert_eq!(sf.read_sample(0, 0, 1), 0.5);
        assert_eq!(sf.read_sample(1, 0, 1), 1.0);
        // The wrap is generated code: the runtime does not duplicate channels.
        assert_eq!(sf.read_sample(2, 0, 1), 0.0);
    }

    #[test]
    fn impulse_test_memory_reader_matches_cpp_fixture_first_samples() {
        let sf = Soundfile::impulse_test_memory_reader(2);

        assert_eq!(sf.num_channels, 2);
        assert_eq!(sf.num_parts, 2);
        assert_eq!(sf.lengths[0], 4096);
        assert_eq!(sf.sample_rates[0], 44100);
        assert_eq!(sf.offsets[1], 4096);
        assert_eq!(sf.buffers.len(), 2);
        assert!(sf.read_sample(0, 0, 0).abs() < f64::EPSILON);
        let expected = (2.0 * std::f64::consts::PI / 4096.0).sin();
        assert!((sf.read_sample(0, 0, 1) - expected).abs() < 1e-15);
        // Channel 1 has its own phase, as in the C++ `TestMemoryReader`.
        let expected = (1.0 + 2.0 * std::f64::consts::PI / 4096.0).sin();
        assert!((sf.read_sample(1, 0, 1) - expected).abs() < 1e-15);
    }
}
