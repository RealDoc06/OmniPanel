//! Prints the mixer state, or changes a volume and holds it.
//!
//! ```text
//! cargo run -p omni-audio --example mixer
//! cargo run -p omni-audio --example mixer -- app <app> <volume|mute|unmute>... [--hold <secs>]
//! cargo run -p omni-audio --example mixer -- master|mic <volume|mute|unmute> [--hold <secs>]
//! ```
//!
//! App levels (and a software master) on macOS live in this process, so each value
//! is held for `--hold` seconds (default 5 for apps, 0 otherwise) and resets when the
//! example exits.

use std::time::{Duration, Instant};

use omni_audio::{AudioController, AudioError, MixerState};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let audio = AudioController::new();
    let result = match args.first().map(String::as_str) {
        None => {
            print_mixer(&audio.mixer());
            for (label, state) in [("master", audio.master()), ("mic", audio.mic())] {
                if let Err(e) = state {
                    println!("{label}: {e}");
                }
            }
            Ok(())
        }
        Some("app") if args.len() >= 3 => run_app(&audio, &args[1], &args[2..]),
        Some(target @ ("master" | "mic")) if args.len() >= 2 => run_endpoint(&audio, target, &args[1..]),
        _ => Err("usage: mixer [app <app> | master | mic] <volume|mute|unmute>... [--hold <secs>]".into()),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

enum Change {
    Volume(f32),
    Muted(bool),
}

fn parse(value: &str) -> Result<Change, String> {
    match value {
        "mute" => Ok(Change::Muted(true)),
        "unmute" => Ok(Change::Muted(false)),
        v => v.parse().map(Change::Volume).map_err(|_| format!("not a volume: {v}")),
    }
}

/// Splits `values... [--hold <secs>]`.
fn split_hold(args: &[String], default: Duration) -> Result<(&[String], Duration), String> {
    match args.iter().position(|a| a == "--hold") {
        Some(i) => {
            let secs: f64 = args.get(i + 1).and_then(|s| s.parse().ok()).ok_or("--hold needs seconds")?;
            Ok((&args[..i], Duration::from_secs_f64(secs)))
        }
        None => Ok((args, default)),
    }
}

fn run_endpoint(audio: &AudioController, target: &str, args: &[String]) -> Result<(), String> {
    let (values, hold) = split_hold(args, Duration::ZERO)?;
    for value in values {
        let result = match (target, parse(value)?) {
            ("master", Change::Volume(v)) => audio.set_master_volume(v),
            ("master", Change::Muted(m)) => audio.set_master_muted(m),
            (_, Change::Volume(v)) => audio.set_mic_volume(v),
            (_, Change::Muted(m)) => audio.set_mic_muted(m),
        };
        result.map_err(|e| e.to_string())?;
        let state = if target == "master" { audio.master() } else { audio.mic() };
        println!("{target} <- {value}: {state:?}");
        std::thread::sleep(hold);
    }
    Ok(())
}

fn run_app(audio: &AudioController, app: &str, args: &[String]) -> Result<(), String> {
    let (values, hold) = split_hold(args, Duration::from_secs(5))?;
    for value in values {
        let change = parse(value)?;
        let apply = || match change {
            Change::Volume(v) => audio.set_app_volume(app, v),
            Change::Muted(m) => audio.set_app_muted(app, m),
        };
        let started = Instant::now();
        let mut result = apply();
        println!("{app} <- {value}: {result:?} in {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
        // First use on macOS: wait for the System Audio Recording prompt to be answered.
        let deadline = Instant::now() + Duration::from_secs(60);
        while matches!(result, Err(AudioError::Permission(_))) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(1));
            result = apply();
        }
        if let Err(e) = result {
            println!("  capabilities: {:?}", audio.capabilities());
            return Err(e.to_string());
        }
        println!("  now: {:?}", audio.app(app));
        std::thread::sleep(hold);
    }
    Ok(())
}

fn print_mixer(mixer: &MixerState) {
    let level = |s: Option<omni_audio::VolumeState>| match s {
        Some(s) => format!("{:>3.0}%{}", s.volume * 100.0, if s.muted { " (muted)" } else { "" }),
        None => "unavailable".into(),
    };
    println!("master  {}", level(mixer.master));
    println!("mic     {}", level(mixer.mic));
    println!("per-app {} {}", mixer.capabilities.per_app_volume, mixer.capabilities.note.as_deref().unwrap_or(""));
    for app in &mixer.apps {
        println!(
            "  {:>3.0}%{} {} {:<28} {} (pid {})",
            app.volume * 100.0,
            if app.muted { " muted" } else { "" },
            if app.playing { '>' } else { ' ' },
            app.name,
            app.id,
            app.pid.map_or("-".into(), |p| p.to_string()),
        );
    }
}
