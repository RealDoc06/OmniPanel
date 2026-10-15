//! Run the voice engine live, for manual testing.
//!
//! cargo run --release -p omni-voice --example live -- --list
//! cargo run --release -p omni-voice --example live -- [preset] [--output <id>] [--monitor] [--secs N]
//!
//! `--list` only enumerates devices. Running live opens the microphone (macOS
//! asks for permission once) and cycles through every built-in preset when no
//! preset is given.

use std::time::{Duration, Instant};

use omni_core::VoiceSettings;
use omni_core::defaults::builtin_voice_presets;
use omni_voice::VoiceEngine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = VoiceEngine::new();
    let mut settings = VoiceSettings { enabled: true, ..VoiceSettings::default() };
    let mut preset_id = None;
    let mut secs = 20u64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => {
                let devices = engine.devices()?;
                for (kind, list) in [("input", devices.inputs), ("output", devices.outputs)] {
                    for d in list {
                        let flags = format!(
                            "{}{}",
                            if d.is_default { " default" } else { "" },
                            if d.is_virtual { " virtual" } else { "" }
                        );
                        println!("{kind:<6} {:<40} {}{flags}", d.name, d.id);
                    }
                }
                println!("{:?}", engine.status());
                return Ok(());
            }
            "--output" => settings.output_device = args.next(),
            "--monitor" => settings.monitor = true,
            "--secs" => secs = args.next().ok_or("--secs N")?.parse()?,
            id => preset_id = Some(id.to_owned()),
        }
    }

    let presets = builtin_voice_presets();
    let selected: Vec<_> = presets.iter().filter(|p| preset_id.as_ref().is_none_or(|id| &p.id == id)).collect();
    let started = Instant::now();
    let per_preset = Duration::from_secs(secs) / selected.len().max(1) as u32;
    for preset in &selected {
        settings.preset_id.clone_from(&preset.id);
        if let Err(e) = engine.apply(&settings, preset) {
            println!("apply failed: {e}\n{:?}", engine.status());
            return Ok(());
        }
        println!("{:?}", engine.status());
        let until = Instant::now() + per_preset;
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(250));
            let l = engine.levels();
            println!(
                "{:>6.1}s  {:<10} in {:>6.1} dB  out {:>6.1} dB",
                started.elapsed().as_secs_f32(),
                preset.id,
                db(l.input),
                db(l.output)
            );
        }
    }
    engine.apply(&VoiceSettings::default(), selected[0])?;
    Ok(())
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-6).log10()
}
