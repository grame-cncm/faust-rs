//! Writing a render to a file, for `--out FILE`.
//!
//! The text dump is the probe's default output and the wrong one for a long
//! render read by a script: a 4 s response of a 16-output program is 176 400
//! rows parsed one by one, each number through a decimal string. `--out` writes
//! the rendered window in binary, at the width the program was compiled in,
//! streamed as it is rendered.
//!
//! Formats, told apart by the extension, as [`crate::probe::audio_file`] does
//! for reading:
//!
//! - `.npy`: NumPy's format, version 1.0, shape `(frames, outputs)`, `<f8` or
//!   `<f4`. `numpy.load` returns the array the CSV parse used to build.
//! - `.wav`: IEEE float, 64 or 32 bits, any channel count, with the sample
//!   rate. `--in file:` reads it back, which is how a render becomes the
//!   excitation or the reference of another.
//! - `.f64`, `.f32`: raw little-endian samples of **one** output, the layout
//!   `--in file:` reads. `.f32` is refused for a double-precision program: it
//!   would drop digits in silence.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Npy,
    Wav,
    Raw,
}

/// A file receiving the frames of one render, in order.
#[derive(Debug)]
pub struct SampleWriter {
    path: PathBuf,
    file: BufWriter<File>,
    /// Whether samples are stored as `f64` (else `f32`).
    wide: bool,
    channels: usize,
    expected: usize,
    written: usize,
    error: Option<String>,
}

impl SampleWriter {
    /// Creates `path` for `frames` frames of `channels` outputs of a program
    /// compiled in double (`double`) or single precision, at `sample_rate`.
    ///
    /// # Errors
    /// An unknown extension, a raw format asked for several outputs, `.f32`
    /// for a double-precision program, a WAV larger than the format allows,
    /// or the file system's refusal.
    pub fn create(
        path: &Path,
        channels: usize,
        frames: usize,
        double: bool,
        sample_rate: i32,
    ) -> Result<Self, String> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let (container, wide) = match extension.as_str() {
            "npy" => (Container::Npy, double),
            "wav" | "wave" => (Container::Wav, double),
            "f64" => (Container::Raw, true),
            "f32" if double => {
                return Err(format!(
                    "{}: a .f32 file would drop digits of a double-precision render; use .f64, .npy or .wav",
                    path.display()
                ));
            }
            "f32" => (Container::Raw, false),
            other => {
                return Err(format!(
                    "{}: unknown output extension `{other}` (expected .npy, .wav, .f64 or .f32)",
                    path.display()
                ));
            }
        };
        if container == Container::Raw && channels != 1 {
            return Err(format!(
                "{}: a raw file holds one channel and the program has {channels} outputs; use .npy or .wav",
                path.display()
            ));
        }
        let bytes_per_sample = if wide { 8 } else { 4 };
        let header = match container {
            Container::Npy => npy_header(frames, channels, wide),
            Container::Wav => wav_header(frames, channels, bytes_per_sample, sample_rate)
                .map_err(|e| format!("{}: {e}", path.display()))?,
            Container::Raw => Vec::new(),
        };
        let file =
            File::create(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
        let mut file = BufWriter::new(file);
        file.write_all(&header)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(Self {
            path: path.to_owned(),
            file,
            wide,
            channels,
            expected: frames,
            written: 0,
            error: None,
        })
    }

    /// Appends one frame. A write error is kept for [`SampleWriter::finish`],
    /// so that the render loop's callback stays infallible.
    pub fn push(&mut self, samples: &[f64]) {
        if self.error.is_some() {
            return;
        }
        debug_assert_eq!(samples.len(), self.channels, "output arity mismatch");
        for &value in samples {
            let result = if self.wide {
                self.file.write_all(&value.to_le_bytes())
            } else {
                // exact: a single-precision program's samples are `f32`
                self.file.write_all(&(value as f32).to_le_bytes())
            };
            if let Err(error) = result {
                self.error = Some(format!("cannot write {}: {error}", self.path.display()));
                return;
            }
        }
        self.written += 1;
    }

    /// Flushes the file and checks that the announced frames were written:
    /// the headers carry the count, and a short file with a long header reads
    /// as garbage.
    ///
    /// # Errors
    /// The first write error, a flush error, or a frame count that differs
    /// from the one announced at creation.
    pub fn finish(mut self) -> Result<(), String> {
        if let Some(error) = self.error {
            return Err(error);
        }
        self.file
            .flush()
            .map_err(|e| format!("cannot write {}: {e}", self.path.display()))?;
        if self.written != self.expected {
            return Err(format!(
                "{}: {} frames written, {} announced",
                self.path.display(),
                self.written,
                self.expected
            ));
        }
        Ok(())
    }
}

/// NumPy format 1.0: magic, version, little-endian `u16` header length, then
/// a Python dict literal padded with spaces and ended by a newline so that
/// the data starts on a multiple of 64 bytes.
fn npy_header(frames: usize, channels: usize, wide: bool) -> Vec<u8> {
    let descr = if wide { "<f8" } else { "<f4" };
    let mut dict = format!(
        "{{'descr': '{descr}', 'fortran_order': False, 'shape': ({frames}, {channels}), }}"
    );
    let unpadded = 10 + dict.len() + 1;
    dict.push_str(&" ".repeat(unpadded.next_multiple_of(64) - unpadded));
    dict.push('\n');
    let mut header = b"\x93NUMPY\x01\x00".to_vec();
    header.extend_from_slice(&(dict.len() as u16).to_le_bytes());
    header.extend_from_slice(dict.as_bytes());
    header
}

/// RIFF/WAVE, `WAVE_FORMAT_IEEE_FLOAT` (3): an 18-byte `fmt ` chunk (a
/// non-PCM format carries its `cbSize`), the `fact` chunk such a format
/// requires, then `data`.
fn wav_header(
    frames: usize,
    channels: usize,
    bytes_per_sample: usize,
    sample_rate: i32,
) -> Result<Vec<u8>, String> {
    let block_align = channels * bytes_per_sample;
    let data_len = frames * block_align;
    let riff_len = 4 + (8 + 18) + (8 + 4) + (8 + data_len);
    let too_large = || "the render is larger than a WAV file can hold (4 GiB); use .npy".to_owned();
    let riff_len = u32::try_from(riff_len).map_err(|_| too_large())?;
    let channels = u16::try_from(channels).map_err(|_| "too many channels for WAV".to_owned())?;
    let rate = u32::try_from(sample_rate).map_err(|_| "negative sample rate".to_owned())?;

    let mut header = Vec::with_capacity(58);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&riff_len.to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&18_u32.to_le_bytes());
    header.extend_from_slice(&3_u16.to_le_bytes());
    header.extend_from_slice(&channels.to_le_bytes());
    header.extend_from_slice(&rate.to_le_bytes());
    header.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
    header.extend_from_slice(&(block_align as u16).to_le_bytes());
    header.extend_from_slice(&((bytes_per_sample * 8) as u16).to_le_bytes());
    header.extend_from_slice(&0_u16.to_le_bytes());
    header.extend_from_slice(b"fact");
    header.extend_from_slice(&4_u32.to_le_bytes());
    header.extend_from_slice(&(frames as u32).to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&(data_len as u32).to_le_bytes());
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::SampleWriter;
    use crate::probe::audio_file::read_channels;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("faustprobe_out_{}_{name}", std::process::id()))
    }

    /// The frames of a two-output render with values no short text holds.
    fn frames() -> Vec<[f64; 2]> {
        (0..5)
            .map(|k| [f64::from(k) / 3.0, -1.0e-7 / f64::from(k + 1)])
            .collect()
    }

    #[test]
    fn a_wav_is_read_back_bit_for_bit_by_the_existing_reader() {
        // The reader is independent code, written for `--in file:`.
        for (double, name) in [(true, "wide.wav"), (false, "narrow.wav")] {
            let path = temp(name);
            let mut writer = SampleWriter::create(&path, 2, 5, double, 48_000).unwrap();
            for frame in frames() {
                writer.push(&frame);
            }
            writer.finish().unwrap();
            let (channels, rate) = read_channels(&path).unwrap();
            let _ = std::fs::remove_file(&path);
            assert_eq!(rate, Some(48_000));
            assert_eq!(channels.len(), 2);
            for (k, frame) in frames().iter().enumerate() {
                for (ch, &value) in frame.iter().enumerate() {
                    let expected = if double {
                        value
                    } else {
                        f64::from(value as f32)
                    };
                    assert_eq!(channels[ch][k].to_bits(), expected.to_bits());
                }
            }
        }
    }

    #[test]
    fn a_raw_file_is_read_back_by_the_existing_reader() {
        let path = temp("mono.f64");
        let mut writer = SampleWriter::create(&path, 1, 3, false, 44_100).unwrap();
        for value in [0.25, -1.0 / 3.0, 1.0e-9] {
            writer.push(&[f64::from(value as f32)]);
        }
        writer.finish().unwrap();
        let (channels, rate) = read_channels(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(rate, None);
        assert_eq!(channels[0].len(), 3);
        assert_eq!(
            channels[0][1].to_bits(),
            f64::from(-1.0_f32 / 3.0).to_bits()
        );
    }

    #[test]
    fn an_npy_file_has_the_documented_layout() {
        // Decoded here from the format's specification, not with the writer's
        // helpers: magic, version 1.0, header length, a dict, then the data on
        // a 64-byte boundary, row-major.
        let path = temp("pair.npy");
        let mut writer = SampleWriter::create(&path, 2, 5, true, 44_100).unwrap();
        for frame in frames() {
            writer.push(&frame);
        }
        writer.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(&bytes[..8], b"\x93NUMPY\x01\x00");
        let header_len = usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        let data_start = 10 + header_len;
        assert_eq!(data_start % 64, 0);
        let dict = std::str::from_utf8(&bytes[10..data_start]).unwrap();
        assert!(dict.ends_with('\n'));
        assert!(dict.contains("'descr': '<f8'"), "{dict}");
        assert!(dict.contains("'fortran_order': False"), "{dict}");
        assert!(dict.contains("'shape': (5, 2)"), "{dict}");

        let data = &bytes[data_start..];
        assert_eq!(data.len(), 5 * 2 * 8);
        let stored = data
            .as_chunks::<8>()
            .0
            .iter()
            .map(|bytes| f64::from_le_bytes(*bytes));
        let expected = frames().into_iter().flatten();
        for (value, expected) in stored.zip(expected) {
            assert_eq!(value.to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn what_would_lose_information_is_refused() {
        let error = |name: &str, channels: usize, double: bool| {
            SampleWriter::create(&temp(name), channels, 1, double, 44_100).unwrap_err()
        };
        assert!(error("x.f32", 1, true).contains("drop digits"));
        assert!(error("x.f64", 2, true).contains("one channel"));
        assert!(error("x.txt", 1, true).contains("unknown output extension"));
    }

    #[test]
    fn a_short_render_is_not_passed_off_as_the_announced_one() {
        let path = temp("short.npy");
        let mut writer = SampleWriter::create(&path, 1, 4, true, 44_100).unwrap();
        writer.push(&[1.0]);
        let error = writer.finish().unwrap_err();
        let _ = std::fs::remove_file(&path);
        assert!(error.contains("1 frames written, 4 announced"), "{error}");
    }
}
