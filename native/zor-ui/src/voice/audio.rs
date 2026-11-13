//! The one audio format the app works in: mono f32 at 24 kHz. Providers hand over whatever rate and channel
//! count they produce; `normalize` brings it here, or says the audio is invalid.

use super::contract::ProviderError;

pub const INTERNAL_RATE: u32 = 24_000;

/// Downmixes interleaved `channels` to mono, resamples linearly to `INTERNAL_RATE` and clamps to -1..1.
/// Linear resampling is enough for speech; it would not do for music.
pub fn normalize(samples: &[f32], rate: u32, channels: u16) -> Result<Vec<f32>, ProviderError> {
    if rate == 0 { return Err(ProviderError::InvalidAudio("sample rate is 0".into())); }
    if channels == 0 { return Err(ProviderError::InvalidAudio("channel count is 0".into())); }
    let ch = channels as usize;
    if !samples.len().is_multiple_of(ch) {
        return Err(ProviderError::InvalidAudio(format!("{} samples do not split into {ch} channels", samples.len())));
    }
    if samples.iter().any(|s| !s.is_finite()) {
        return Err(ProviderError::InvalidAudio("audio holds NaN or infinite samples".into()));
    }
    let mono: Vec<f32> = samples.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    let out_len = (mono.len() as u64 * INTERNAL_RATE as u64 / rate as u64) as usize;
    let step = rate as f64 / INTERNAL_RATE as f64;
    Ok((0..out_len).map(|i| {
        let pos = i as f64 * step;
        let i0 = pos.floor() as usize;
        let frac = (pos - i0 as f64) as f32;
        let a = mono[i0.min(mono.len() - 1)];
        let b = mono[(i0 + 1).min(mono.len() - 1)];
        (a + (b - a) * frac).clamp(-1.0, 1.0)
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_mono_passes_through() {
        let s = vec![0.0, 0.5, -0.5, 1.0];
        assert_eq!(normalize(&s, INTERNAL_RATE, 1).unwrap(), s);
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        // L=1, R=0 then L=0, R=1 -> both frames mono 0.5
        let out = normalize(&[1.0, 0.0, 0.0, 1.0], INTERNAL_RATE, 2).unwrap();
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn half_rate_doubles_the_length() {
        let out = normalize(&[0.0, 1.0], INTERNAL_RATE / 2, 1).unwrap();
        assert_eq!(out.len(), 4);
        assert!((out[1] - 0.5).abs() < 1e-6, "midpoint interpolated, got {}", out[1]);
    }

    #[test]
    fn values_are_clamped() {
        assert_eq!(normalize(&[3.0, -3.0], INTERNAL_RATE, 1).unwrap(), vec![1.0, -1.0]);
    }

    #[test]
    fn bad_input_is_invalid_audio() {
        assert!(matches!(normalize(&[0.0], 0, 1), Err(ProviderError::InvalidAudio(_))));
        assert!(matches!(normalize(&[0.0], 16_000, 0), Err(ProviderError::InvalidAudio(_))));
        assert!(matches!(normalize(&[0.0, 0.0, 0.0], 16_000, 2), Err(ProviderError::InvalidAudio(_))), "odd sample count");
        assert!(matches!(normalize(&[f32::NAN], 16_000, 1), Err(ProviderError::InvalidAudio(_))));
    }
}
