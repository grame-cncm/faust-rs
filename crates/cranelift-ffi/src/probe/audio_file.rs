//! Reading an excitation from a file, for `--in file:PATH[:CH]`.
//!
//! Three formats, told apart by the extension: `.wav` (RIFF/WAVE, PCM 8, 16,
//! 24 or 32-bit integer and 32 or 64-bit float, any channel count), `.f64`
//! and `.f32` (raw little-endian samples, one channel, what
//! `scripts/make_target.py` of faust-diff-fdn writes). Samples come back as
//! `f64` in `[-1, 1]` for integer formats, as stored for float ones.

use std::path::Path;

/// The channels of a file, each a vector of samples, and its sample rate
/// when the format records one (a WAV file; the raw formats do not).
pub fn read_channels(path: &Path) -> Result<(Vec<Vec<f64>>, Option<u32>), String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !matches!(extension.as_str(), "wav" | "wave" | "f64" | "f32") {
        return Err(format!(
            "{}: unknown audio extension `{extension}` (expected .wav, .f64 or .f32)",
            path.display()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    match extension.as_str() {
        "wav" | "wave" => read_wav(&bytes).map_err(|e| format!("{}: {e}", path.display())),
        "f64" => Ok((
            vec![
                bytes
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| f64::from_le_bytes(*c))
                    .collect(),
            ],
            None,
        )),
        "f32" => Ok((
            vec![
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f64::from(f32::from_le_bytes(*c)))
                    .collect(),
            ],
            None,
        )),
        _ => unreachable!("extension checked above"),
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, String> {
    bytes
        .get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| "truncated WAV header".to_owned())
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, String> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| "truncated WAV header".to_owned())
}

/// A RIFF/WAVE file: the `fmt ` chunk (PCM, IEEE float, or the extensible
/// header wrapping either), then the `data` chunk; the channels and the
/// sample rate.
fn read_wav(bytes: &[u8]) -> Result<(Vec<Vec<f64>>, Option<u32>), String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_owned());
    }
    let (mut format, mut channels, mut bits, mut rate) = (0u16, 0u16, 0u16, 0u32);
    let mut data: Option<&[u8]> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(bytes, at + 4)? as usize;
        let body = bytes
            .get(at + 8..(at + 8 + size).min(bytes.len()))
            .unwrap_or(&[]);
        match id {
            b"fmt " => {
                format = u16_at(body, 0)?;
                channels = u16_at(body, 2)?;
                rate = u32_at(body, 4)?;
                bits = u16_at(body, 14)?;
                if format == 0xFFFE && body.len() >= 26 {
                    // WAVE_FORMAT_EXTENSIBLE: the real format is the sub-format's first field
                    format = u16_at(body, 24)?;
                }
            }
            b"data" => {
                data = Some(body);
                break;
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    let data = data.ok_or_else(|| "no data chunk".to_owned())?;
    if channels == 0 {
        return Err("no fmt chunk, or zero channels".to_owned());
    }
    let channels = usize::from(channels);
    let bytes_per_sample = usize::from(bits / 8);
    let frame = channels * bytes_per_sample;
    if frame == 0 {
        return Err(format!("unsupported bit depth {bits}"));
    }
    let decode: fn(&[u8]) -> f64 = match (format, bits) {
        (1, 8) => |b| (f64::from(b[0]) - 128.0) / 128.0,
        (1, 16) => |b| f64::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0,
        (1, 24) => |b| f64::from(i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) / 8_388_608.0,
        (1, 32) => |b| f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])) / 2_147_483_648.0,
        (3, 32) => |b| f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        (3, 64) => |b| f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        _ => return Err(format!("unsupported WAV format {format} at {bits} bits")),
    };
    let frames = data.len() / frame;
    let mut out = vec![Vec::with_capacity(frames); channels];
    for f in data.chunks_exact(frame) {
        for (ch, sample) in f.chunks_exact(bytes_per_sample).enumerate() {
            out[ch].push(decode(sample));
        }
    }
    Ok((out, (rate != 0).then_some(rate)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(format: u16, bits: u16, channels: u16, data: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&format.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&44_100u32.to_le_bytes());
        let block = u32::from(channels) * u32::from(bits / 8);
        b.extend_from_slice(&(44_100 * block).to_le_bytes());
        b.extend_from_slice(&(block as u16).to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(data);
        b
    }

    #[test]
    fn decodes_pcm16_stereo_and_float32() {
        let mut data = Vec::new();
        for v in [16384i16, -32768, 0, 32767] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let (channels, rate) = read_wav(&wav(1, 16, 2, &data)).unwrap();
        assert_eq!(rate, Some(44100));
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0], [0.5, 0.0]);
        assert_eq!(channels[1], [-1.0, 32767.0 / 32768.0]);
        let mut data = Vec::new();
        for v in [0.25f32, -0.75] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(read_wav(&wav(3, 32, 1, &data)).unwrap().0, [[0.25, -0.75]]);
    }

    #[test]
    fn decodes_pcm24() {
        // +1/2 full scale (0x400000) then -1/2 (0xC00000), little-endian 3 bytes
        let data = [0x00, 0x00, 0x40, 0x00, 0x00, 0xC0];
        assert_eq!(read_wav(&wav(1, 24, 1, &data)).unwrap().0, [[0.5, -0.5]]);
    }

    #[test]
    fn reads_raw_f64_by_extension() {
        let dir = std::env::temp_dir().join(format!("faustprobe-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.f64");
        let mut bytes = Vec::new();
        for v in [1.0f64, -0.5, 0.125] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(
            read_channels(&path).unwrap(),
            (vec![vec![1.0, -0.5, 0.125]], None)
        );
        assert!(
            read_channels(&dir.join("t.mp3"))
                .unwrap_err()
                .contains("unknown audio extension")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_non_wav_bytes() {
        assert!(
            read_wav(b"not a wave file at all")
                .unwrap_err()
                .contains("RIFF")
        );
    }
}
