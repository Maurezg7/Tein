//! Five-band equalizer applied to the decoded audio.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, SampleRate, Source};

pub const BANDS: usize = 5;
const FREQS: [f32; BANDS] = [60.0, 230.0, 910.0, 3600.0, 12000.0];
pub const MAX_DB: f32 = 12.0;

pub const PRESETS: [(&str, [f32; BANDS]); 7] = [
    ("Plano", [0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Graves", [7.0, 4.0, 0.0, 0.0, 0.0]),
    ("Agudos", [0.0, 0.0, 0.0, 4.0, 7.0]),
    ("Rock", [5.0, 2.0, -2.0, 3.0, 5.0]),
    ("Pop", [-1.0, 2.0, 4.0, 2.0, -1.0]),
    ("Voz", [-3.0, 0.0, 4.0, 4.0, 0.0]),
    ("Electrónica", [6.0, 3.0, 0.0, 2.0, 5.0]),
];

// Gains in tenths of a decibel, shared with the audio thread.
static GAINS: [AtomicI32; BANDS] = [const { AtomicI32::new(0) }; BANDS];
static VERSION: AtomicU32 = AtomicU32::new(1);

pub fn set_gains(gains: [f32; BANDS]) {
    for (slot, g) in GAINS.iter().zip(gains) {
        slot.store((g.clamp(-MAX_DB, MAX_DB) * 10.0) as i32, Ordering::Relaxed);
    }
    VERSION.fetch_add(1, Ordering::Release);
}

fn gains() -> [f32; BANDS] {
    std::array::from_fn(|i| GAINS[i].load(Ordering::Relaxed) as f32 / 10.0)
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Biquad {
    /// Peaking filter from the RBJ audio EQ cookbook.
    fn peaking(freq: f32, gain_db: f32, rate: f32) -> Self {
        let freq = freq.min(rate * 0.45);
        let a = 10f32.powf(gain_db / 40.0);
        let w = 2.0 * std::f32::consts::PI * freq / rate;
        let alpha = w.sin() / 2.0;
        let a0 = 1.0 + alpha / a;
        Biquad {
            b0: (1.0 + alpha * a) / a0,
            b1: -2.0 * w.cos() / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: -2.0 * w.cos() / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }
}

pub struct Equalizer<S> {
    input: S,
    version: u32,
    channels: usize,
    rate: u32,
    channel: usize,
    active: [bool; BANDS],
    preamp: f32,
    filters: [Biquad; BANDS],
    /// Per channel and band: the two delay values of a transposed direct form II.
    state: Vec<[[f32; 2]; BANDS]>,
}

impl<S: Source> Equalizer<S> {
    pub fn new(input: S) -> Self {
        Equalizer {
            input,
            version: 0,
            channels: 0,
            rate: 0,
            channel: 0,
            active: [false; BANDS],
            preamp: 1.0,
            filters: [Biquad::default(); BANDS],
            state: vec![],
        }
    }

    fn configure(&mut self, version: u32) {
        let channels = self.input.channels().get() as usize;
        let rate = self.input.sample_rate().get();
        if channels != self.channels || rate != self.rate {
            self.state = vec![[[0.0; 2]; BANDS]; channels];
            self.channel = 0;
        }
        (self.channels, self.rate, self.version) = (channels, rate, version);
        let gains = gains();
        for i in 0..BANDS {
            self.active[i] = gains[i].abs() >= 0.05;
            self.filters[i] = Biquad::peaking(FREQS[i], gains[i], rate as f32);
        }
        // Leave headroom for the loudest boost so it does not clip.
        let boost = gains.iter().cloned().fold(0.0f32, f32::max);
        self.preamp = 10f32.powf(-boost / 20.0);
    }
}

impl<S: Source> Iterator for Equalizer<S> {
    type Item = S::Item;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let mut x = self.input.next()?;
        if self.channel == 0 {
            let version = VERSION.load(Ordering::Acquire);
            if version != self.version
                || self.input.channels().get() as usize != self.channels
                || self.input.sample_rate().get() != self.rate
            {
                self.configure(version);
            }
        }
        let state = &mut self.state[self.channel];
        self.channel = (self.channel + 1) % self.channels;
        x *= self.preamp;
        for i in 0..BANDS {
            if !self.active[i] {
                continue;
            }
            let f = &self.filters[i];
            let s = &mut state[i];
            let y = f.b0 * x + s[0];
            s[0] = f.b1 * x - f.a1 * y + s[1];
            s[1] = f.b2 * x - f.a2 * y;
            x = y;
        }
        Some(x)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.input.size_hint()
    }
}

impl<S: Source> Source for Equalizer<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.input.current_span_len()
    }
    fn channels(&self) -> ChannelCount {
        self.input.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.input.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        for channel in &mut self.state {
            *channel = [[0.0; 2]; BANDS];
        }
        self.input.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(freq: f32, gains: [f32; BANDS]) -> f32 {
        set_gains(gains);
        let rate = 44100;
        let samples: Vec<f32> = (0..rate)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.1)
            .collect();
        let source = rodio::buffer::SamplesBuffer::new(
            ChannelCount::new(1).unwrap(),
            SampleRate::new(rate).unwrap(),
            samples,
        );
        let out: Vec<f32> = Equalizer::new(source).collect();
        (out[rate as usize / 2..].iter().map(|s| s * s).sum::<f32>() / (rate as f32 / 2.0)).sqrt()
    }

    #[test]
    fn boosts_only_the_chosen_band() {
        let flat = level(60.0, [0.0; BANDS]);
        // The preamp takes 6 dB off everything, so the boosted band stays level
        // and the untouched ones drop.
        let bass = level(60.0, [6.0, 0.0, 0.0, 0.0, 0.0]);
        let treble = level(12000.0, [6.0, 0.0, 0.0, 0.0, 0.0]);
        assert!((bass / flat - 1.0).abs() < 0.05, "{bass} vs {flat}");
        assert!((treble / flat - 0.5).abs() < 0.05, "{treble} vs {flat}");
        set_gains([0.0; BANDS]);
    }
}
