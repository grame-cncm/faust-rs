//! `--describe`: what a render sounds like, as a few numbers.
//!
//! The descriptors of a sound-space atlas (`faust-elements`'
//! `scripts/gen_atlas.mjs`) and of a port checked against its original, where
//! two implementations cannot be compared sample by sample but their
//! envelopes and their brightness can. FFI-free, so unit-testable.
//!
//! # Definitions
//! Every descriptor reads the **window** of the render (the frames after
//! `--skip`), and every time is counted in seconds from its first frame: a
//! note meant to be measured starts there.
//!
//! - The **mid** signal is the mean of the outputs, `m = (x0 + ... + xN-1) / N`.
//! - Its **envelope** is the mean of `m²` over consecutive frames of `hop`
//!   seconds (rounded to whole samples), from the window's start; a last
//!   partial frame is left out. The loudest frame is the first largest one.
//! - `loudness`: the loudest frame, `10 log10(e_peak)` dBFS (`-inf` for a
//!   silent window). A full-scale sine on every output reads -3.01 dB.
//! - `attack`: the start of the first frame whose energy reaches `0.81
//!   e_peak` (90 % in amplitude). Resolution: one hop; 0 means "within the
//!   first hop".
//! - `decay:X` (`decay` is `decay:30`): from the loudest frame to the first
//!   later frame at or below `e_peak · 10^(-X/10)`, in whole hops. `NaN` when
//!   the window ends first: not reached, which is not the window's length.
//! - `sustain:A..B`: the mean energy of the frames lying wholly within
//!   `[A, B)`, in dB against the loudest frame.
//! - `centroid[:A..B]`: the spectral centroid of the mid in Hz, over `[A, B)`
//!   (default the whole window): power spectra of `fft` samples, periodic
//!   Hann window, hop `fft/2`, summed; bins 2 to `fft/2 - 1` (DC and the bin
//!   beside it out, Nyquist out). `NaN` when that band holds no energy.
//! - `width[:A..B]`: side against mid energy of a program with two outputs,
//!   `10 log10(Σ(x0-x1)²) - 10 log10(Σ(x0+x1)²)` over `[A, B)` (default the
//!   whole window). `-inf` for two identical outputs.
//!
//! These are `describe()` of `gen_atlas.mjs`, with the same hop, window and
//! bands, except where a number would have hidden a fact there: a decay not
//! reached is `NaN` (not the render's end), and no level is floored.

use std::fmt;

use super::spectrum::fft;

/// Envelope hop and transform length: the parameters every descriptor shares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DescribeSettings {
    /// Envelope frame, seconds (`--describe-hop`).
    pub hop_seconds: f64,
    /// Transform length of `centroid`, a power of two (`--describe-fft`).
    pub fft: usize,
}

impl Default for DescribeSettings {
    fn default() -> Self {
        Self {
            hop_seconds: 0.01,
            fft: 2048,
        }
    }
}

/// A stretch of the window, in seconds from its first frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub from: f64,
    pub to: f64,
}

impl Range {
    /// The samples `[from, to)` at `sr`.
    fn samples(self, sr: f64) -> (usize, usize) {
        (
            seconds_to_samples(self.from, sr),
            seconds_to_samples(self.to, sr),
        )
    }
}

/// `seconds · sr` rounded to whole samples; ranges are checked non-negative
/// before they are used.
fn seconds_to_samples(seconds: f64, sr: f64) -> usize {
    (seconds * sr).round().max(0.0) as usize
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.from, self.to)
    }
}

/// One descriptor of `--describe`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Descriptor {
    Loudness,
    Attack,
    /// Fall from the loudest frame, dB.
    Decay(f64),
    Sustain(Range),
    Centroid(Option<Range>),
    Width(Option<Range>),
}

impl fmt::Display for Descriptor {
    /// The canonical spelling, which heads the descriptor's column: the same
    /// descriptor always has the same name (`decay` is `decay:30`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loudness => f.write_str("loudness"),
            Self::Attack => f.write_str("attack"),
            Self::Decay(db) => write!(f, "decay:{db}"),
            Self::Sustain(range) => write!(f, "sustain:{range}"),
            Self::Centroid(None) => f.write_str("centroid"),
            Self::Centroid(Some(range)) => write!(f, "centroid:{range}"),
            Self::Width(None) => f.write_str("width"),
            Self::Width(Some(range)) => write!(f, "width:{range}"),
        }
    }
}

/// The names `--describe` accepts, for its messages.
const ACCEPTED: &str = "loudness, attack, decay[:DB], sustain:A..B, centroid[:A..B], width[:A..B]";

/// Parse a `--describe` list: descriptors separated by commas.
///
/// # Errors
/// An unknown name (with the accepted ones), a malformed parameter, an empty
/// list or a descriptor given twice.
pub fn parse_descriptors(text: &str) -> Result<Vec<Descriptor>, String> {
    let mut list: Vec<Descriptor> = Vec::new();
    for item in text.split(',') {
        let item = item.trim();
        if item.is_empty() {
            return Err(format!("empty descriptor in `{text}`; expected {ACCEPTED}"));
        }
        let descriptor = parse_descriptor(item)?;
        if list.contains(&descriptor) {
            return Err(format!("`{descriptor}` is given twice in `{text}`"));
        }
        list.push(descriptor);
    }
    Ok(list)
}

fn parse_descriptor(item: &str) -> Result<Descriptor, String> {
    let (name, parameter) = match item.split_once(':') {
        Some((name, parameter)) => (name, Some(parameter)),
        None => (item, None),
    };
    let range = |parameter: &str| parse_range(item, parameter);
    match (name, parameter) {
        ("loudness", None) => Ok(Descriptor::Loudness),
        ("attack", None) => Ok(Descriptor::Attack),
        ("decay", None) => Ok(Descriptor::Decay(30.0)),
        ("decay", Some(db)) => {
            let db: f64 = db
                .parse()
                .map_err(|_| format!("`{db}` is not a number of dB in `{item}`"))?;
            if !(db.is_finite() && db > 0.0) {
                return Err(format!(
                    "`{item}`: the fall must be a positive number of dB"
                ));
            }
            Ok(Descriptor::Decay(db))
        }
        ("sustain", Some(parameter)) => Ok(Descriptor::Sustain(range(parameter)?)),
        ("sustain", None) => Err(
            "`sustain` needs the stretch it measures, in seconds from the window's start: \
             `sustain:A..B`"
                .to_owned(),
        ),
        ("centroid", parameter) => Ok(Descriptor::Centroid(parameter.map(range).transpose()?)),
        ("width", parameter) => Ok(Descriptor::Width(parameter.map(range).transpose()?)),
        ("loudness" | "attack", Some(_)) => {
            Err(format!("`{name}` takes no parameter, got `{item}`"))
        }
        _ => Err(format!("unknown descriptor `{item}`; expected {ACCEPTED}")),
    }
}

fn parse_range(item: &str, parameter: &str) -> Result<Range, String> {
    let (from, to) = parameter
        .split_once("..")
        .ok_or_else(|| format!("expected A..B (seconds) in `{item}`"))?;
    let number = |text: &str| -> Result<f64, String> {
        text.trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("`{text}` is not a number of seconds in `{item}`"))
    };
    let range = Range {
        from: number(from)?,
        to: number(to)?,
    };
    if !(range.from >= 0.0 && range.from < range.to) {
        return Err(format!("`{item}`: the stretch must satisfy 0 <= A < B"));
    }
    Ok(range)
}

/// Checks `descriptors` against what a run will render, before it renders:
/// `outputs` channels, a window of `window` samples at `sr`.
///
/// # Errors
/// `width` on a program that does not have two outputs, a stretch that ends
/// beyond the window or holds no whole frame (no envelope frame for
/// `sustain`, fewer samples than one transform for `centroid`), an envelope
/// hop shorter than a sample, or a transform length that is not a power of
/// two of at least 8.
pub fn check(
    descriptors: &[Descriptor],
    settings: DescribeSettings,
    outputs: usize,
    window: usize,
    sr: f64,
) -> Result<(), String> {
    if !(settings.hop_seconds.is_finite() && settings.hop_seconds > 0.0) {
        return Err("--describe-hop must be a positive number of seconds".to_owned());
    }
    let hop = hop_samples(settings, sr);
    if hop == 0 {
        return Err(format!(
            "--describe-hop {} is shorter than one sample at {sr} Hz",
            settings.hop_seconds
        ));
    }
    if !(settings.fft.is_power_of_two() && settings.fft >= 8) {
        return Err(format!(
            "--describe-fft {} must be a power of two of at least 8",
            settings.fft
        ));
    }
    if outputs == 0 {
        return Err("--describe needs a program with outputs".to_owned());
    }
    let duration = window as f64 / sr;
    let within = |descriptor: &Descriptor, range: Range| -> Result<(usize, usize), String> {
        let (from, to) = range.samples(sr);
        if to > window {
            return Err(format!(
                "`{descriptor}` ends at {} s, beyond the window of {duration} s ({window} frames)",
                range.to
            ));
        }
        Ok((from, to))
    };
    for descriptor in descriptors {
        match *descriptor {
            Descriptor::Loudness | Descriptor::Attack | Descriptor::Decay(_) => {
                if window < hop {
                    return Err(format!(
                        "`{descriptor}` needs one envelope frame ({hop} samples); \
                         the window has {window}"
                    ));
                }
            }
            Descriptor::Sustain(range) => {
                let (from, to) = within(descriptor, range)?;
                if frames_within(from, to, hop).is_empty() {
                    return Err(format!(
                        "`{descriptor}` holds no whole envelope frame of {hop} samples"
                    ));
                }
            }
            Descriptor::Centroid(range) => {
                let (from, to) = match range {
                    Some(range) => within(descriptor, range)?,
                    None => (0, window),
                };
                if to - from < settings.fft {
                    return Err(format!(
                        "`{descriptor}` covers {} samples, fewer than one transform of {} \
                         (--describe-fft)",
                        to - from,
                        settings.fft
                    ));
                }
            }
            Descriptor::Width(range) => {
                if outputs != 2 {
                    return Err(format!(
                        "`{descriptor}` compares two outputs; this program has {outputs}"
                    ));
                }
                if let Some(range) = range {
                    within(descriptor, range)?;
                }
            }
        }
    }
    Ok(())
}

fn hop_samples(settings: DescribeSettings, sr: f64) -> usize {
    seconds_to_samples(settings.hop_seconds, sr)
}

/// The envelope frames lying wholly within the samples `[from, to)`.
fn frames_within(from: usize, to: usize, hop: usize) -> std::ops::Range<usize> {
    from.div_ceil(hop)..to / hop
}

/// The values of `descriptors` on the window `channels` (one `Vec` per
/// output, all of one length) at `sr`, in their order. Call [`check`] first:
/// this assumes what it verified.
#[must_use]
pub fn describe(
    channels: &[Vec<f64>],
    sr: f64,
    descriptors: &[Descriptor],
    settings: DescribeSettings,
) -> Vec<f64> {
    let mid = mid(channels);
    let hop = hop_samples(settings, sr);
    let envelope = envelope(&mid, hop);
    // the first largest frame
    let (peak_frame, peak) =
        envelope.iter().enumerate().fold(
            (0, 0.0_f64),
            |best, (k, e)| if *e > best.1 { (k, *e) } else { best },
        );
    let seconds = |frames: usize| (frames * hop) as f64 / sr;
    descriptors
        .iter()
        .map(|descriptor| match *descriptor {
            Descriptor::Loudness => db(peak),
            Descriptor::Attack => {
                if peak > 0.0 {
                    let frame = envelope.iter().position(|e| *e >= 0.81 * peak);
                    frame.map_or(f64::NAN, seconds)
                } else {
                    f64::NAN
                }
            }
            Descriptor::Decay(fall) => {
                if peak > 0.0 {
                    let floor = peak * 10f64.powf(-fall / 10.0);
                    envelope[peak_frame..]
                        .iter()
                        .position(|e| *e <= floor)
                        .map_or(f64::NAN, seconds)
                } else {
                    f64::NAN
                }
            }
            Descriptor::Sustain(range) => {
                let (from, to) = range.samples(sr);
                let frames = frames_within(from, to, hop);
                let count = frames.len() as f64;
                let mean = envelope[frames].iter().sum::<f64>() / count;
                if peak > 0.0 {
                    db(mean) - db(peak)
                } else {
                    f64::NAN
                }
            }
            Descriptor::Centroid(range) => {
                let (from, to) = range.map_or((0, mid.len()), |r| r.samples(sr));
                centroid(&mid[from..to], sr, settings.fft)
            }
            Descriptor::Width(range) => {
                let (from, to) = range.map_or((0, mid.len()), |r| r.samples(sr));
                width(&channels[0][from..to], &channels[1][from..to])
            }
        })
        .collect()
}

/// `10 log10(energy)`, `-inf` for zero.
fn db(energy: f64) -> f64 {
    10.0 * energy.log10()
}

/// The mean of the outputs, frame by frame.
fn mid(channels: &[Vec<f64>]) -> Vec<f64> {
    let n = channels.len() as f64;
    let len = channels.first().map_or(0, Vec::len);
    (0..len)
        .map(|i| channels.iter().map(|c| c[i]).sum::<f64>() / n)
        .collect()
}

/// Mean square of `signal` over consecutive frames of `hop` samples.
fn envelope(signal: &[f64], hop: usize) -> Vec<f64> {
    signal
        .chunks_exact(hop)
        .map(|frame| frame.iter().map(|x| x * x).sum::<f64>() / hop as f64)
        .collect()
}

/// Spectral centroid of `signal`: summed Hann-windowed power spectra of `n`
/// samples, hop `n/2`, over bins `2..n/2`.
fn centroid(signal: &[f64], sr: f64, n: usize) -> f64 {
    let hann: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        .collect();
    let mut power = vec![0.0_f64; n / 2];
    let mut re = vec![0.0_f64; n];
    let mut im = vec![0.0_f64; n];
    let mut start = 0;
    while start + n <= signal.len() {
        for i in 0..n {
            re[i] = hann[i] * signal[start + i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for (k, p) in power.iter_mut().enumerate() {
            *p += re[k] * re[k] + im[k] * im[k];
        }
        start += n / 2;
    }
    let bin = sr / n as f64;
    let (mut total, mut weighted) = (0.0_f64, 0.0_f64);
    for (k, p) in power.iter().enumerate().skip(2) {
        total += p;
        weighted += p * k as f64 * bin;
    }
    if total > 0.0 {
        weighted / total
    } else {
        f64::NAN
    }
}

/// Side against mid energy of two outputs, dB.
fn width(left: &[f64], right: &[f64]) -> f64 {
    let (mut side, mut mid) = (0.0_f64, 0.0_f64);
    for (l, r) in left.iter().zip(right) {
        side += (l - r) * (l - r);
        mid += (l + r) * (l + r);
    }
    db(side) - db(mid)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 32_000.0;

    fn sine(n: usize, hz: f64, amplitude: f64) -> Vec<f64> {
        (0..n)
            .map(|i| amplitude * (std::f64::consts::TAU * hz * i as f64 / SR).sin())
            .collect()
    }

    fn one(channels: &[Vec<f64>], descriptor: Descriptor) -> f64 {
        let settings = DescribeSettings::default();
        let window = channels[0].len();
        check(&[descriptor], settings, channels.len(), window, SR).unwrap();
        describe(channels, SR, &[descriptor], settings)[0]
    }

    #[test]
    fn canonical_names_round_trip() {
        let list = parse_descriptors(
            "loudness, attack,decay,decay:60,sustain:0.8..1,centroid,centroid:0.05..1,width:0..1",
        )
        .unwrap();
        let names: Vec<String> = list.iter().map(ToString::to_string).collect();
        assert_eq!(
            names,
            [
                "loudness",
                "attack",
                "decay:30",
                "decay:60",
                "sustain:0.8..1",
                "centroid",
                "centroid:0.05..1",
                "width:0..1"
            ]
        );
        for name in &names {
            assert_eq!(parse_descriptors(name).unwrap().len(), 1, "{name}");
        }
    }

    #[test]
    fn refuses_malformed_lists() {
        for text in [
            "",
            "loudness,",
            "brightness",
            "sustain",
            "sustain:1..0.5",
            "sustain:-1..1",
            "centroid:0.1",
            "decay:-3",
            "decay:x",
            "attack:3",
            "decay,decay:30",
        ] {
            assert!(parse_descriptors(text).is_err(), "`{text}` accepted");
        }
    }

    #[test]
    fn a_full_scale_sine_is_three_db_down() {
        let x = sine(32_000, 1000.0, 1.0);
        let loudness = one(&[x.clone(), x], Descriptor::Loudness);
        assert!(
            (loudness - (-3.010_299_956_639_812)).abs() < 1e-9,
            "{loudness}"
        );
    }

    #[test]
    fn the_centroid_of_a_sine_on_a_bin_is_its_frequency() {
        // bin 64 of 2048 at 32 kHz: 1000 Hz exactly; Hann leaks into bins
        // 63 and 65 symmetrically, so the centroid stays on 1000 Hz
        let x = sine(32_000, 1000.0, 0.5);
        let centroid = one(&[x], Descriptor::Centroid(None));
        assert!((centroid - 1000.0).abs() < 1e-6, "{centroid}");
    }

    #[test]
    fn the_centroid_of_two_equal_sines_is_their_mean() {
        let x: Vec<f64> = sine(32_000, 1000.0, 0.5)
            .iter()
            .zip(sine(32_000, 3000.0, 0.5))
            .map(|(a, b)| a + b)
            .collect();
        let centroid = one(&[x], Descriptor::Centroid(None));
        assert!((centroid - 2000.0).abs() < 1e-6, "{centroid}");
    }

    #[test]
    fn an_exponential_decay_takes_its_time_constant() {
        // energy falls 60 dB per second: 30 dB in 0.5 s, in 10 ms frames
        let x: Vec<f64> = (0..64_000)
            .map(|i| 10f64.powf(-3.0 * i as f64 / SR) * if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let decay = one(std::slice::from_ref(&x), Descriptor::Decay(30.0));
        // frame 50 sits on the floor itself, which rounding may put on
        // either side
        assert!(decay == 0.5 || decay == 0.51, "{decay}");
        let sustain = one(
            &[x],
            Descriptor::Sustain(Range {
                from: 1.0,
                to: 1.01,
            }),
        );
        // frame 100 against frame 0: 60 dB less
        assert!((sustain - -60.0).abs() < 1e-9, "{sustain}");
    }

    #[test]
    fn a_decay_not_reached_is_nan() {
        let x = sine(32_000, 1000.0, 0.5);
        assert!(one(&[x], Descriptor::Decay(30.0)).is_nan());
    }

    #[test]
    fn a_linear_ramp_reaches_ninety_percent_on_its_frame() {
        // amplitude ramps 0 -> 1 over 1 s: 0.9 is reached at 0.9 s, and the
        // frame that starts there holds the first energy above 0.81 of the
        // peak frame's (which is the last one, 0.99..1 s)
        let x: Vec<f64> = (0..32_000)
            .map(|i| (i as f64 / 32_000.0) * if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let attack = one(&[x], Descriptor::Attack);
        assert!((attack - 0.9).abs() <= 0.01, "{attack}");
    }

    #[test]
    fn width_reads_side_against_mid() {
        let x = sine(3200, 500.0, 0.5);
        assert_eq!(
            one(&[x.clone(), x.clone()], Descriptor::Width(None)),
            f64::NEG_INFINITY
        );
        let opposite: Vec<f64> = x.iter().map(|v| -v).collect();
        assert_eq!(
            one(&[x.clone(), opposite], Descriptor::Width(None)),
            f64::INFINITY
        );
        // right at half the left's level: side 0.25, mid 2.25 (in x²)
        let half: Vec<f64> = x.iter().map(|v| 0.5 * v).collect();
        let width = one(&[x, half], Descriptor::Width(None));
        assert!(
            (width - 10.0 * (0.25f64 / 2.25).log10()).abs() < 1e-12,
            "{width}"
        );
    }

    #[test]
    fn silence_has_no_attack_and_no_centroid() {
        let x = vec![0.0; 4096];
        assert_eq!(
            one(std::slice::from_ref(&x), Descriptor::Loudness),
            f64::NEG_INFINITY
        );
        assert!(one(std::slice::from_ref(&x), Descriptor::Attack).is_nan());
        assert!(one(&[x], Descriptor::Centroid(None)).is_nan());
    }

    #[test]
    fn check_refuses_what_cannot_be_measured() {
        let s = DescribeSettings::default();
        let width = Descriptor::Width(None);
        assert!(check(&[width], s, 1, 32_000, SR).is_err());
        let late = Descriptor::Sustain(Range { from: 0.5, to: 2.0 });
        assert!(check(&[late], s, 1, 32_000, SR).is_err());
        let thin = Descriptor::Sustain(Range {
            from: 0.001,
            to: 0.015,
        });
        assert!(check(&[thin], s, 1, 32_000, SR).is_err());
        assert!(check(&[Descriptor::Centroid(None)], s, 1, 1000, SR).is_err());
        let odd = DescribeSettings { fft: 1000, ..s };
        assert!(check(&[Descriptor::Loudness], odd, 1, 32_000, SR).is_err());
        let tiny = DescribeSettings {
            hop_seconds: 1e-6,
            ..s
        };
        assert!(check(&[Descriptor::Loudness], tiny, 1, 32_000, SR).is_err());
    }
}
