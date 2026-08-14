//! Soundfile fixture for the reference impulse-test protocol.
//!
//! A DSP declaring `soundfile(...)` receives a pointer the host must fill in.
//! When nothing does, the pointer stays null and the first `compute`
//! dereferences it — which is a segfault, not a diagnostic. Both the impulse
//! runner and the probe therefore install this in-memory reader, matching the
//! C++ suite's own fixture so the rendered `.ir` is comparable.
//!
//! Shared rather than duplicated: the buffer sizes and the channel-sharing
//! layout are numerically load-bearing, and two copies would drift.

use std::ffi::c_void;

#[repr(C)]
#[derive(Debug)]
pub struct RawSoundfile {
    buffers: *mut c_void,
    lengths: *mut i32,
    sample_rates: *mut i32,
    offsets: *mut i32,
    channels: i32,
    parts: i32,
    is_double: bool,
}

#[derive(Debug)]
pub struct TestSoundfile {
    raw: Box<RawSoundfile>,
    #[allow(dead_code)]
    pub(crate) lengths: Vec<i32>,
    #[allow(dead_code)]
    pub(crate) sample_rates: Vec<i32>,
    #[allow(dead_code)]
    pub(crate) offsets: Vec<i32>,
    #[allow(dead_code)]
    pub(crate) channel_ptrs: Vec<*mut f64>,
    #[allow(dead_code)]
    pub(crate) buffers: Vec<Vec<f64>>,
}

impl TestSoundfile {
    #[must_use]
    pub fn impulse_test_memory_reader(num_real_parts: usize) -> Self {
        const SOUND_CHAN: usize = 2;
        const SOUND_LENGTH: usize = 4096;
        const SOUND_SR: i32 = 44100;
        const BUFFER_SIZE: usize = 1024;
        const MAX_CHAN: usize = 64;
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
                let value = (part as f64
                    + (2.0 * std::f64::consts::PI * sample as f64 / SOUND_LENGTH as f64))
                    .sin();
                for channel in buffers.iter_mut().take(SOUND_CHAN) {
                    channel[part_offset + sample] = value;
                }
            }
        }

        let mut channel_ptrs = Vec::with_capacity(MAX_CHAN);
        for channel in 0..MAX_CHAN {
            channel_ptrs.push(buffers[channel % SOUND_CHAN].as_mut_ptr());
        }

        let raw = Box::new(RawSoundfile {
            buffers: channel_ptrs.as_mut_ptr().cast::<c_void>(),
            lengths: lengths.as_mut_ptr(),
            sample_rates: sample_rates.as_mut_ptr(),
            offsets: offsets.as_mut_ptr(),
            channels: SOUND_CHAN as i32,
            parts: real_parts as i32,
            is_double: true,
        });

        Self {
            raw,
            lengths,
            sample_rates,
            offsets,
            channel_ptrs,
            buffers,
        }
    }

    /// Pointer the DSP's soundfile zone must be set to.
    pub fn as_mut_ptr(&mut self) -> *mut c_void {
        self.raw.as_mut() as *mut RawSoundfile as *mut c_void
    }
}

/// Counts the resource parts encoded in a Faust soundfile URL.
#[must_use]
pub fn soundfile_part_count(url: &str) -> usize {
    let trimmed = url.trim();
    let Some(open) = trimmed.find('{') else {
        return usize::from(!trimmed.is_empty()).max(1);
    };
    let Some(close) = trimmed[open + 1..].find('}') else {
        return 1;
    };
    let body = &trimmed[open + 1..open + 1 + close];
    let count = body
        .split(';')
        .filter(|part| !part.trim().trim_matches('\'').is_empty())
        .count();
    count.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soundfile_part_count_follows_sound_ui_menu_urls() {
        assert_eq!(soundfile_part_count("{'sound1';'sound2'}"), 2);
        assert_eq!(soundfile_part_count("sound1"), 1);
        assert_eq!(soundfile_part_count(""), 1);
    }

    #[test]
    fn test_soundfile_shares_channels_like_cpp_fixture() {
        let mut sf = TestSoundfile::impulse_test_memory_reader(2);
        assert_eq!(sf.lengths[0], 4096);
        assert_eq!(sf.offsets[1], 4096);
        assert_eq!(sf.channel_ptrs[0], sf.buffers[0].as_mut_ptr());
        assert_eq!(sf.channel_ptrs[2], sf.buffers[0].as_mut_ptr());
    }
}
