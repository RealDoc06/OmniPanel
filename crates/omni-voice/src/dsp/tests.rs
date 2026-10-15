use std::f32::consts::TAU;

use omni_core::defaults::builtin_voice_presets;
use omni_core::{Effect, FilterKind};

use super::*;

const SR: f32 = 48_000.0;

fn sine(freq: f32, amp: f32, secs: f32) -> Vec<f32> {
    (0..(secs * SR) as usize).map(|i| amp * (TAU * freq * i as f32 / SR).sin()).collect()
}

/// Deterministic white noise in `[-amp, amp]`.
fn noise(amp: f32, secs: f32) -> Vec<f32> {
    let mut state = 0x2545_f491_u32;
    (0..(secs * SR) as usize)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            amp * (state as f32 / u32::MAX as f32 * 2.0 - 1.0)
        })
        .collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

/// Frequency from interpolated rising zero crossings.
fn zero_crossing_freq(x: &[f32]) -> f32 {
    let crossings: Vec<f32> = x
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
        .map(|(i, w)| i as f32 + w[0] / (w[0] - w[1]))
        .collect();
    let (first, last) = (crossings[0], crossings[crossings.len() - 1]);
    (crossings.len() - 1) as f32 * SR / (last - first)
}

fn run_chain(effects: &[Effect], mut input: Vec<f32>) -> Vec<f32> {
    let mut chain = Chain::new(effects, SR);
    for block in input.chunks_mut(480) {
        chain.process(block);
    }
    input
}

#[test]
fn pitch_shift_moves_frequency_by_ratio() {
    for semitones in [-12.0f32, -5.0, 3.0, 7.0, 12.0] {
        let out = run_chain(&[Effect::Pitch { semitones }], sine(220.0, 0.5, 2.0));
        let measured = zero_crossing_freq(&out[(0.5 * SR) as usize..]);
        let expected = 220.0 * 2f32.powf(semitones / 12.0);
        let error = (measured / expected - 1.0).abs();
        assert!(error < 0.005, "{semitones} st: {measured:.2} Hz, expected {expected:.2} Hz");
        // Aligned splices keep the level steady (no comb-filter dips).
        let level = rms(&out[(0.5 * SR) as usize..]);
        assert!((level / (0.5 / 2f32.sqrt()) - 1.0).abs() < 0.1, "{semitones} st: rms {level}");
    }
}

#[test]
fn pitch_shift_splices_in_phase() {
    // Misaligned splices partially cancel during the crossfade, so the
    // envelope of a shifted sine dips. Exact envelope of a discrete sine:
    // sqrt(y[n]² + ((y[n+1] - y[n-1]) / (2 sin ω))²).
    for (semitones, hz) in [(-7.0f32, 150.0), (-7.0, 200.0), (7.0, 150.0), (7.0, 200.0), (12.0, 247.0)] {
        let out = run_chain(&[Effect::Pitch { semitones }], sine(hz, 0.5, 1.5));
        let w = TAU * hz * 2f32.powf(semitones / 12.0) / SR;
        let (lo, hi) = out[(0.2 * SR) as usize..].windows(3).fold((1.0f32, 0.0f32), |(lo, hi), y| {
            let env = (y[1] * y[1] + ((y[2] - y[0]) / (2.0 * w.sin())).powi(2)).sqrt();
            (lo.min(env), hi.max(env))
        });
        assert!(lo > 0.45 && hi < 0.55, "{semitones} st @ {hz} Hz: envelope {lo}..{hi}");
    }
}

#[test]
fn filters_attenuate_as_expected() {
    let gain_db = |kind, cutoff, q, freq| {
        let mut out = sine(freq, 0.5, 0.5);
        Biquad::new(kind, cutoff, q, SR).process(&mut out);
        20.0 * (rms(&out[(0.25 * SR) as usize..]) / (0.5 / 2f32.sqrt())).log10()
    };
    assert!(gain_db(FilterKind::Lowpass, 1000.0, 0.707, 100.0).abs() < 0.5);
    assert!(gain_db(FilterKind::Lowpass, 1000.0, 0.707, 8000.0) < -30.0);
    assert!((gain_db(FilterKind::Lowpass, 1000.0, 0.707, 1000.0) + 3.0).abs() < 0.5);
    assert!(gain_db(FilterKind::Highpass, 1000.0, 0.707, 100.0) < -35.0);
    assert!(gain_db(FilterKind::Highpass, 1000.0, 0.707, 8000.0).abs() < 0.5);
    assert!(gain_db(FilterKind::Bandpass, 1500.0, 0.9, 1500.0).abs() < 0.5);
    assert!(gain_db(FilterKind::Bandpass, 1500.0, 0.9, 100.0) < -20.0);
    assert!(gain_db(FilterKind::Bandpass, 1500.0, 0.9, 15000.0) < -15.0);
}

#[test]
fn gate_closes_on_silence_and_does_not_chatter() {
    let mut gate = Gate::new(SR);
    gate.threshold = Some(db_to_gain(-40.0));

    let mut speech = sine(300.0, db_to_gain(-20.0), 0.5);
    gate.process(&mut speech);
    assert!(gate.is_open());
    assert!(rms(&speech[(0.1 * SR) as usize..]) > 0.99 * db_to_gain(-20.0) / 2f32.sqrt());

    let mut hiss = noise(db_to_gain(-70.0), 1.0);
    gate.process(&mut hiss);
    assert!(!gate.is_open());
    let tail = &hiss[(0.3 * SR) as usize..];
    assert!(tail.iter().all(|v| v.abs() < db_to_gain(-90.0)), "gate leaks noise");
    assert!(tail[tail.len() - 4800..].iter().all(|&v| v == 0.0), "gate never fully closes");

    // Level wobbling between -38 and -44 dB around the -40 dB threshold:
    // opens once and stays open thanks to hysteresis.
    let mut transitions = 0;
    let mut was_open = gate.is_open();
    let mut wobble: Vec<f32> = (0..(2.0 * SR) as usize)
        .map(|i| {
            let t = i as f32 / SR;
            let db = -41.0 + 3.0 * (TAU * 7.0 * t).sin();
            db_to_gain(db) * (TAU * 250.0 * t).sin()
        })
        .collect();
    for block in wobble.chunks_mut(64) {
        gate.process(block);
        if gate.is_open() != was_open {
            transitions += 1;
            was_open = gate.is_open();
        }
    }
    assert_eq!(transitions, 1);
}

#[test]
fn builtin_presets_stay_finite_and_bounded() {
    let loud_sine = sine(220.0, 0.95, 3.0);
    let loud_noise = noise(0.9, 3.0);
    for preset in builtin_voice_presets() {
        for (name, input) in [("sine", &loud_sine), ("noise", &loud_noise)] {
            let mut p = Processor::new(SR, Chain::new(&preset.effects, SR));
            p.set_params(Params { input_gain: db_to_gain(12.0), output_gain: db_to_gain(12.0), gate_threshold: None });
            let mut out = input.clone();
            for block in out.chunks_mut(512) {
                p.process(block);
            }
            assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0), "{} / {name}", preset.id);
            assert!(rms(&out[out.len() / 2..]) > 0.01, "{} / {name} went silent", preset.id);
        }
    }
}

#[test]
fn extreme_parameters_stay_bounded() {
    let effects = vec![
        Effect::Gain { db: 24.0 },
        Effect::Flanger { rate_hz: 10.0, depth_ms: 10.0, feedback: 0.95, mix: 1.0 },
        Effect::Echo { delay_ms: 1.0, feedback: 0.95, mix: 1.0 },
        Effect::Reverb { room_size: 1.0, damping: 0.0, mix: 1.0 },
        Effect::Filter { kind: FilterKind::Bandpass, frequency: 30_000.0, q: f32::NAN },
        Effect::Pitch { semitones: 40.0 },
        Effect::Robot { frequency: 0.0, mix: 0.5 },
        Effect::Distortion { drive: 5.0, mix: 1.0 },
        Effect::Bitcrush { bits: 0, downsample: 255 },
    ];
    let mut p = Processor::new(SR, Chain::new(&effects, SR));
    let mut out = noise(1.0, 3.0);
    p.process(&mut out);
    assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
}

#[test]
fn preset_swap_mid_stream_is_click_free() {
    let presets = builtin_voice_presets();
    let effects = |id: &str| presets.iter().find(|p| p.id == id).unwrap().effects.clone();
    for (from, to) in [("clean", "deep"), ("chipmunk", "clean"), ("deep", "chipmunk"), ("clean", "echo")] {
        let input = sine(200.0, 0.4, 2.0);
        let mut p = Processor::new(SR, Chain::new(&effects(from), SR));
        let mut out = input.clone();
        let swap_at = 187 * 256; // ~1 s, on a block boundary
        for (i, block) in out.chunks_mut(256).enumerate() {
            if i * 256 == swap_at {
                p.set_chain(Box::new(Chain::new(&effects(to), SR)));
            }
            p.process(block);
        }
        let max_step = |x: &[f32]| x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        let settle = (0.3 * SR) as usize;
        let before = max_step(&out[settle..swap_at]);
        let after = max_step(&out[swap_at + settle..]);
        let during = max_step(&out[swap_at - 16..swap_at + settle]);
        assert!(during <= 1.25 * before.max(after) + 1e-3, "{from} -> {to}: step {during} vs {before} / {after}");
    }
}

#[test]
fn preset_swap_before_history_is_full() {
    // The replay window starts "below" index 0 of the history ring (which happens
    // whenever the write position is < the replay length), here before it filled once.
    let mut p = Processor::new(SR, Chain::new(&[], SR));
    let mut out = sine(200.0, 0.4, 0.5);
    let (first, rest) = out.split_at_mut(256);
    p.process(first);
    p.set_chain(Box::new(Chain::new(&[Effect::Echo { delay_ms: 50.0, feedback: 0.3, mix: 0.5 }], SR)));
    for block in rest.chunks_mut(256) {
        p.process(block);
    }
    assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
}

#[test]
fn robot_has_fixed_pitch() {
    // Two different input pitches, same robot pitch.
    for input_hz in [140.0f32, 230.0] {
        let saw: Vec<f32> =
            (0..(1.5 * SR) as usize).map(|i| 0.5 * ((input_hz * i as f32 / SR).fract() * 2.0 - 1.0)).collect();
        let out = run_chain(&[Effect::Robot { frequency: 110.0, mix: 1.0 }], saw);
        let x = &out[(0.5 * SR) as usize..];
        let lag = (SR as usize / 400..SR as usize / 60)
            .max_by(|&a, &b| {
                let ac = |l: usize| x[..x.len() - l].iter().zip(&x[l..]).map(|(p, q)| p * q).sum::<f32>();
                ac(a).total_cmp(&ac(b))
            })
            .unwrap();
        let hz = SR / lag as f32;
        assert!((hz - 110.0).abs() < 1.0, "input {input_hz} Hz -> robot {hz} Hz");
        assert!(rms(x) > 0.05, "robot output too quiet: {}", rms(x));
    }
}
