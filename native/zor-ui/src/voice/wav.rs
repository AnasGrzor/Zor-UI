//! Reads 16-bit PCM WAV bytes into f32 samples at their own rate and channel count. Sidecars write WAV; the
//! result goes through `audio::normalize`.

use super::contract::ProviderError;

pub struct Wav {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

pub fn parse(b: &[u8]) -> Result<Wav, ProviderError> {
    let bad = |m: &str| ProviderError::InvalidAudio(m.to_string());
    let u16at = |i: usize| b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32at = |i: usize| b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    if b.get(0..4) != Some(b"RIFF") || b.get(8..12) != Some(b"WAVE") { return Err(bad("not a WAV file")); }
    let (mut i, mut fmt, mut data) = (12usize, None, None);
    while let (Some(id), Some(len)) = (b.get(i..i + 4), u32at(i + 4)) {
        let body = i + 8..(i + 8 + len as usize).min(b.len());
        match id {
            b"fmt " => fmt = Some((u16at(body.start), u16at(body.start + 2), u32at(body.start + 4), u16at(body.start + 14))),
            b"data" => data = Some(body.clone()),
            _ => {}
        }
        i = body.end + (len as usize & 1);
    }
    let (Some((Some(tag), Some(ch), Some(rate), Some(bits))), Some(data)) = (fmt, data) else {
        return Err(bad("WAV has no fmt or data chunk"));
    };
    if tag != 1 || bits != 16 { return Err(bad("only 16-bit PCM WAV is supported")); }
    if ch == 0 || rate == 0 { return Err(bad("WAV has zero channels or rate")); }
    let samples = b[data].chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0).collect();
    Ok(Wav { rate, channels: ch, samples })
}

/// A 16-bit PCM WAV file with `pcm` as its samples.
pub fn encode(rate: u32, channels: u16, pcm: &[i16]) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    v.extend_from_slice(&(channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm { v.extend_from_slice(&s.to_le_bytes()); }
    v
}

/// f32 samples (-1 to 1) as 16-bit PCM, clamped.
pub fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rate_channels_and_scaled_samples() {
        let w = parse(&encode(22_050, 2, &[16384, -16384, 0, 32767])).unwrap();
        assert_eq!((w.rate, w.channels), (22_050, 2));
        assert_eq!(w.samples.len(), 4);
        assert!((w.samples[0] - 0.5).abs() < 1e-4 && (w.samples[1] + 0.5).abs() < 1e-4);
    }

    #[test]
    fn rejects_non_wav_and_non_pcm() {
        assert!(matches!(parse(b"nope, not a wav file at all"), Err(ProviderError::InvalidAudio(_))));
        let mut float = encode(16_000, 1, &[0]);
        float[20] = 3; // format tag 3: IEEE float
        assert!(matches!(parse(&float), Err(ProviderError::InvalidAudio(_))));
    }

    #[test]
    fn float_samples_survive_the_round_trip_within_one_step() {
        let src = [0.0f32, 0.25, -0.5, 1.0];
        let w = parse(&encode(24_000, 1, &to_i16(&src))).unwrap();
        for (a, b) in src.iter().zip(&w.samples) { assert!((a - b).abs() < 1.0 / 16384.0, "{a} vs {b}"); }
    }

    #[test]
    fn a_truncated_file_is_invalid_not_a_panic() {
        let full = encode(16_000, 1, &[1, 2, 3, 4]);
        assert!(parse(&full[..30]).is_err());
    }
}
