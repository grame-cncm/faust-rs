//! Reading an excitation from a file, for `--in file:PATH[:CH]`.
//!
//! Four formats, told apart by the extension: `.wav` (RIFF/WAVE, PCM 8, 16,
//! 24 or 32-bit integer and 32 or 64-bit float, any channel count), `.f64`
//! and `.f32` (raw little-endian samples, one channel, what
//! `scripts/make_target.py` of faust-diff-fdn writes), and `.npy` (NumPy,
//! little-endian `<f8` or `<f4`, C order, shape `(frames,)` or `(frames,
//! channels)`: what `--out` and `numpy.save` write). Samples come back as
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
    if !matches!(extension.as_str(), "wav" | "wave" | "f64" | "f32" | "npy") {
        return Err(format!(
            "{}: unknown audio extension `{extension}` (expected .wav, .npy, .f64 or .f32)",
            path.display()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    match extension.as_str() {
        "wav" | "wave" => read_wav(&bytes).map_err(|e| format!("{}: {e}", path.display())),
        "npy" => read_npy(&bytes)
            .map(|channels| (channels, None))
            .map_err(|e| format!("{}: {e}", path.display())),
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

/// A NumPy `.npy` file, format 1.0 to 3.0: the magic, the version, the length
/// of a header that is a Python dict literal, then the data. Read here: the
/// float arrays `--out` and `numpy.save` write, little-endian, C order, one or
/// two dimensions; anything else is refused by name rather than misread.
fn read_npy(bytes: &[u8]) -> Result<Vec<Vec<f64>>, String> {
    if bytes.len() < 10 || &bytes[0..6] != b"\x93NUMPY" {
        return Err("not a NumPy .npy file".to_owned());
    }
    // version 1 has a 16-bit header length, versions 2 and 3 a 32-bit one
    let (header_len, header_at) = match bytes[6] {
        1 => (usize::from(u16::from_le_bytes([bytes[8], bytes[9]])), 10),
        2 | 3 => {
            let len = bytes
                .get(8..12)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .ok_or_else(|| "truncated .npy header".to_owned())?;
            (len as usize, 12)
        }
        other => return Err(format!("unsupported .npy format version {other}")),
    };
    let header = bytes
        .get(header_at..header_at + header_len)
        .and_then(|h| std::str::from_utf8(h).ok())
        .ok_or_else(|| "truncated .npy header".to_owned())?;
    let data = &bytes[header_at + header_len..];

    let field = |key: &str| -> Result<&str, String> {
        let at = header
            .find(key)
            .ok_or_else(|| format!("no {key} in the .npy header"))?;
        Ok(header[at + key.len()..].trim_start_matches([':', ' ']))
    };
    let descr = field("'descr'")?;
    let width = if descr.starts_with("'<f8'") {
        8
    } else if descr.starts_with("'<f4'") {
        4
    } else {
        let shown = descr.split(',').next().unwrap_or(descr);
        return Err(format!(
            "unsupported .npy element type {shown} (expected '<f8' or '<f4')"
        ));
    };
    if field("'fortran_order'")?.starts_with("True") {
        return Err("a Fortran-order .npy array is not supported (save it in C order)".to_owned());
    }
    let shape = field("'shape'")?;
    let inside = shape
        .strip_prefix('(')
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(|| "malformed shape in the .npy header".to_owned())?;
    let dims = inside
        .split(',')
        .map(str::trim)
        .filter(|dim| !dim.is_empty())
        .map(|dim| {
            dim.parse::<usize>()
                .map_err(|_| format!("malformed shape `({inside})`"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (frames, channels) = match dims.as_slice() {
        [frames] => (*frames, 1),
        [frames, channels] => (*frames, *channels),
        _ => {
            return Err(format!(
                "a .npy array of shape ({inside}) is not frames x channels"
            ));
        }
    };
    if data.len() != frames * channels * width {
        return Err(format!(
            "the .npy data holds {} bytes, its shape ({inside}) announces {}",
            data.len(),
            frames * channels * width
        ));
    }
    let sample = |k: usize| -> f64 {
        let at = k * width;
        if width == 8 {
            f64::from_le_bytes(data[at..at + 8].try_into().expect("eight bytes"))
        } else {
            f64::from(f32::from_le_bytes(
                data[at..at + 4].try_into().expect("four bytes"),
            ))
        }
    };
    // row-major: frame by frame
    Ok((0..channels)
        .map(|ch| {
            (0..frames)
                .map(|frame| sample(frame * channels + ch))
                .collect()
        })
        .collect())
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

    /// A `.npy` file as `numpy.save` lays it out, built here from the format's
    /// description and not with the writer of `audio_out`.
    fn npy(version: u8, descr: &str, fortran: bool, shape: &str, data: &[u8]) -> Vec<u8> {
        let order = if fortran { "True" } else { "False" };
        let mut dict =
            format!("{{'descr': '{descr}', 'fortran_order': {order}, 'shape': {shape}, }}");
        let prefix = if version == 1 { 10 } else { 12 };
        while (prefix + dict.len() + 1) % 64 != 0 {
            dict.push(' ');
        }
        dict.push('\n');
        let mut bytes = b"\x93NUMPY".to_vec();
        bytes.extend_from_slice(&[version, 0]);
        if version == 1 {
            bytes.extend_from_slice(&(dict.len() as u16).to_le_bytes());
        } else {
            bytes.extend_from_slice(&(dict.len() as u32).to_le_bytes());
        }
        bytes.extend_from_slice(dict.as_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    #[test]
    fn npy_two_dimensions_are_frames_by_channels_in_row_major_order() {
        // frames (1, 10), (2, 20), (3, 30)
        let data: Vec<u8> = [1.0_f64, 10.0, 2.0, 20.0, 3.0, 30.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        for version in [1, 2] {
            let channels = read_npy(&npy(version, "<f8", false, "(3, 2)", &data)).unwrap();
            assert_eq!(channels, [vec![1.0, 2.0, 3.0], vec![10.0, 20.0, 30.0]]);
        }
    }

    #[test]
    fn npy_one_dimension_is_one_channel_and_f4_is_widened_exactly() {
        let third = 1.0_f32 / 3.0;
        let data: Vec<u8> = [third, -0.5].iter().flat_map(|v| v.to_le_bytes()).collect();
        let channels = read_npy(&npy(1, "<f4", false, "(2,)", &data)).unwrap();
        assert_eq!(channels, [vec![f64::from(third), -0.5]]);
    }

    #[test]
    fn npy_that_would_be_misread_is_refused_by_name() {
        let eight = [0_u8; 8];
        let error = |bytes: &[u8]| read_npy(bytes).unwrap_err();
        assert!(error(&npy(1, "<i4", false, "(2,)", &eight)).contains("'<i4'"));
        assert!(error(&npy(1, ">f8", false, "(1,)", &eight)).contains("'>f8'"));
        assert!(error(&npy(1, "<f8", true, "(1, 1)", &eight)).contains("Fortran"));
        assert!(error(&npy(1, "<f8", false, "(1, 1, 1)", &eight)).contains("frames x channels"));
        // a shape that announces more than the file holds
        assert!(error(&npy(1, "<f8", false, "(2, 1)", &eight)).contains("announces 16"));
        assert!(error(b"RIFF....").contains("not a NumPy"));
    }

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
