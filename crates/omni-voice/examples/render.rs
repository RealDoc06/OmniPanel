//! Offline render of every built-in voice preset, for listening tests, plus a
//! CPU benchmark of each chain.
//!
//! cargo run --release -p omni-voice --example render -- <in.wav> [out_dir]
//!
//! Writes `<out_dir>/omni-voice-<preset>.wav` (mono 16-bit, input rate).

use std::path::PathBuf;
use std::time::Instant;

use omni_core::VoiceSettings;
use omni_core::defaults::builtin_voice_presets;
use omni_voice::dsp::{Chain, Params, Processor};

/// Same block size a 48 kHz device callback typically uses.
const BLOCK: usize = 256;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or("usage: render <in.wav> [out_dir]")?;
    let out_dir = PathBuf::from(args.next().unwrap_or_else(|| "/tmp".into()));

    let (samples, sr) = read_mono(&input)?;
    let secs = samples.len() as f32 / sr;
    println!("{input}: {secs:.1} s @ {sr} Hz");
    let params = Params::from(&VoiceSettings::default());

    for preset in builtin_voice_presets() {
        let mut processor = Processor::new(sr, Chain::new(&preset.effects, sr));
        processor.set_params(params);
        let mut out = samples.clone();
        for block in out.chunks_mut(BLOCK) {
            processor.process(block);
        }
        let path = out_dir.join(format!("omni-voice-{}.wav", preset.id));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: sr as u32,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec)?;
        for s in &out {
            writer.write_sample((s * i16::MAX as f32) as i16)?;
        }
        writer.finalize()?;
        println!("  wrote {}", path.display());
    }

    // Benchmark on 20 s of noise at 48 kHz (worst case for the gate: always open).
    let rate = 48_000.0;
    let mut noise = vec![0.0f32; 20 * rate as usize];
    let mut state = 1u32;
    for s in &mut noise {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *s = (state >> 8) as f32 / (1 << 24) as f32 - 0.5;
    }
    println!("\nCPU (single core, 48 kHz, {BLOCK}-frame blocks):");
    for preset in builtin_voice_presets() {
        let mut processor = Processor::new(rate, Chain::new(&preset.effects, rate));
        let mut buf = noise.clone();
        let start = Instant::now();
        for block in buf.chunks_mut(BLOCK) {
            processor.process(block);
        }
        let elapsed = start.elapsed().as_secs_f64();
        let ns = elapsed * 1e9 / buf.len() as f64;
        println!(
            "  {:<10} {:>7.0}x realtime  {:>6.1} ns/sample  {:>5.2}% of a core",
            preset.id,
            20.0 / elapsed,
            ns,
            ns * rate as f64 / 1e7
        );
    }
    Ok(())
}

fn read_mono(path: &str) -> Result<(Vec<f32>, f32), Box<dyn std::error::Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>()?
        }
    };
    let channels = usize::from(spec.channels);
    let mono = interleaved.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect();
    Ok((mono, spec.sample_rate as f32))
}
