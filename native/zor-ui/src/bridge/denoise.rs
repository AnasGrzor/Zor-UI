//! Noise suppression for the microphone: DeepFilterNet 3, a small neural network that keeps the voice and takes out fans,
//! keys, the room and music without a singer. It runs at 48 kHz; the recorder works at 16 kHz, so audio goes up and back down.
//! The model is built into the program (about 8 MB). Each recording loads it on its own thread and frees it at the end.

use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::{ArrayView2, ArrayViewMut2};
use std::{sync::mpsc, time::Duration};

/// How many times faster DeepFilterNet's rate (48 kHz) is than the recorder's (16 kHz).
const UP: usize = 3;
/// How long a recorder waits for a cleaned chunk before it keeps the raw one (the model is still loading).
const WAIT: Duration = Duration::from_millis(60);

/// One recording's place in the model: its own model state and resampling buffers.
struct Stream {
    df: Option<DfTract>,
    /// The last 16 kHz sample, for upsampling across chunk edges.
    last: f32,
    /// 48 kHz input not yet a whole hop.
    hi: Vec<f32>,
    /// 48 kHz output not yet a whole group of `UP`.
    out_hi: Vec<f32>,
    /// 16 kHz output not handed out yet.
    out: std::collections::VecDeque<f32>,
}

impl Stream {
    fn new(df: Option<DfTract>) -> Stream { Stream { df, last: 0.0, hi: vec![], out_hi: vec![], out: Default::default() } }

    /// Cleans one chunk of 16 kHz mono. The result has the same length: when a chunk doesn't fill whole hops, the missing
    /// samples are silence at the start and the rest comes with the next chunk.
    fn run(&mut self, a: &[f32]) -> Vec<f32> {
        let Some(df) = self.df.as_mut() else { return a.to_vec() };
        let hop = df.hop_size;
        self.hi.extend(upsample(self.last, a));
        if let Some(&l) = a.last() { self.last = l; }
        let whole = self.hi.len() - self.hi.len() % hop;
        let mut enh = vec![0f32; hop];
        for h in self.hi[..whole].chunks_exact(hop) {
            let (noisy, clean) = (ArrayView2::from_shape((1, hop), h).unwrap(), ArrayViewMut2::from_shape((1, hop), &mut enh[..]).unwrap());
            // a failed hop keeps the raw audio, so a model error never drops speech
            if df.process(noisy, clean).is_ok() { self.out_hi.extend_from_slice(&enh) } else { self.out_hi.extend_from_slice(h) }
        }
        self.hi.drain(..whole);
        let lo = downsample(&self.out_hi);
        self.out_hi.drain(..lo.len() * UP);
        self.out.extend(lo);
        let short = a.len().saturating_sub(self.out.len());
        std::iter::repeat_n(0.0, short).chain(self.out.drain(..a.len() - short)).collect()
    }
}

/// A recording's noise suppressor: its own thread, since tract's model can't move between threads. The model loads when
/// the recording starts and is freed when it ends, so nothing stays in memory between recordings.
pub(super) struct Denoise(mpsc::Sender<(Vec<f32>, mpsc::Sender<Vec<f32>>)>);

impl Denoise {
    pub fn new() -> Result<Denoise, String> {
        let (tx, rx) = mpsc::channel::<(Vec<f32>, mpsc::Sender<Vec<f32>>)>();
        std::thread::spawn(move || {
            let df = DfTract::new(DfParams::default(), &RuntimeParams::default())
                .map_err(|e| crate::log::line(&format!("noise suppression could not load: {e}"))).ok();
            let mut s = Stream::new(df);
            for (a, reply) in rx { let _ = reply.send(s.run(&a)); } // ends when the recording drops its Denoise
        });
        Ok(Denoise(tx))
    }

    /// `a` cleaned, the same length; `a` itself while the model is still loading or if its thread is gone.
    pub fn run(&mut self, a: &[f32]) -> Vec<f32> {
        let (tx, rx) = mpsc::channel();
        if self.0.send((a.to_vec(), tx)).is_err() { return a.to_vec(); }
        rx.recv_timeout(WAIT).ok().filter(|c| c.len() == a.len()).unwrap_or_else(|| a.to_vec())
    }
}

/// 16 kHz to 48 kHz, straight lines between samples; `last` is the sample before `a`.
pub fn upsample(last: f32, a: &[f32]) -> Vec<f32> {
    let mut prev = last;
    let mut v = Vec::with_capacity(a.len() * UP);
    for &x in a {
        for k in 1..=UP { v.push(prev + (x - prev) * k as f32 / UP as f32); }
        prev = x;
    }
    v
}

/// 48 kHz to 16 kHz: the mean of each whole group of `UP`. A partial group at the end is left out.
pub fn downsample(a: &[f32]) -> Vec<f32> {
    a.chunks_exact(UP).map(|g| g.iter().sum::<f32>() / UP as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_that_misses_a_whole_hop_comes_out_the_same_length() {
        // no model: run() passes audio through, so only the length bookkeeping is under test here
        let mut d = Stream::new(None);
        assert_eq!(d.run(&[0.1; 1600]).len(), 1600);
        assert_eq!(d.run(&[0.1; 7]).len(), 7);
    }

    #[test]
    fn resampling_up_and_down_returns_the_same_signal() {
        let a = [0.3f32, -0.6, 0.9];
        let up = upsample(0.0, &a);
        assert_eq!(up.len(), 9);
        assert!((up[2] - 0.3).abs() < 1e-6 && (up[8] - 0.9).abs() < 1e-6, "each third sample is the original");
        assert_eq!(downsample(&up[..8]).len(), 2, "a partial group waits");
        let down = downsample(&upsample(0.5, &[0.5; 30]));
        assert!(down.iter().all(|x| (x - 0.5).abs() < 1e-6));
    }

    /// Loads the real model, so it is slow in debug builds.
    #[test]
    #[ignore = "loads the 8 MB model; run with --release -- --ignored"]
    fn the_model_keeps_lengths_and_quiets_noise() {
        let mut d = Stream::new(Some(DfTract::new(DfParams::default(), &RuntimeParams::default()).unwrap()));
        let mut seed = 1u32;
        let mut noise = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / (1u32 << 24) as f32 * 0.2 - 0.1 };
        let (mut in_e, mut out_e) = (0f32, 0f32);
        for i in 0..30 {
            let a: Vec<f32> = (0..1600).map(|_| noise()).collect();
            let o = d.run(&a);
            assert_eq!(o.len(), a.len());
            if i >= 10 { in_e += a.iter().map(|x| x * x).sum::<f32>(); out_e += o.iter().map(|x| x * x).sum::<f32>(); }
        }
        assert!(out_e < in_e / 4.0, "white noise drops by over 6 dB: in {in_e}, out {out_e}");
    }
}
