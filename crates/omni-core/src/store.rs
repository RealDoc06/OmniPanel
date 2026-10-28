//! Loading and saving `config.json` atomically, and the lock that keeps a
//! data folder to one process.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::model::Config;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("config io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config is not valid json: {0}")]
    Json(#[from] serde_json::Error),
}

pub const CONFIG_FILE: &str = "config.json";
pub const LOCK_FILE: &str = "omnipanel.lock";

pub fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONFIG_FILE)
}

/// Exclusive hold on a data folder, so two processes never write the same
/// config. Released when dropped or when the process exits.
#[derive(Debug)]
pub struct DataDirLock {
    _file: fs::File,
}

/// Take the data folder lock. `Ok(None)` means another process holds it.
pub fn lock(data_dir: &Path) -> Result<Option<DataDirLock>, StoreError> {
    fs::create_dir_all(data_dir)?;
    let file = fs::OpenOptions::new().create(true).write(true).truncate(false).open(data_dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(DataDirLock { _file: file })),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Load the config, creating a default one when missing. A file that doesn't
/// parse is kept as `config.json.broken-<ms>` and replaced by what can be
/// salvaged from it, so a bad edit never prevents the app from starting and
/// only costs the records it broke.
pub fn load(data_dir: &Path) -> Result<Config, StoreError> {
    load_with_repair_status(data_dir).map(|(config, _)| config)
}

/// Like [`load`], also reporting whether the file needed repair so the host can warn.
pub fn load_with_repair_status(data_dir: &Path) -> Result<(Config, bool), StoreError> {
    load_with_repair_report(data_dir).map(|(config, warning)| (config, warning.is_some()))
}

/// Also returns a warning with the recovery result and the damaged file's location.
pub fn load_with_repair_report(data_dir: &Path) -> Result<(Config, Option<String>), StoreError> {
    let path = config_path(data_dir);
    let mut repair = None;
    let mut config = match fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Config>(&bytes) {
            Ok(config) => config,
            Err(_) => {
                let backup = path.with_extension(format!("json.broken-{}", crate::now_ms()));
                fs::rename(&path, &backup)?;
                let recovered = serde_json::from_slice(&bytes)
                    .ok()
                    .or_else(|| close_truncated_json(&bytes))
                    .and_then(|value| salvage(value, &|v| v))
                    .and_then(|value| serde_json::from_value(value).ok());
                let recovered_any = recovered.is_some();
                let config: Config = recovered.unwrap_or_default();
                let kept = if recovered_any {
                    format!(
                        "Kept layouts: {}; devices: {}; custom themes: {}; voice presets: {}. Unreadable values were reset to defaults.",
                        config.profiles.len(),
                        config.devices.len(),
                        config.themes.len(),
                        config.voice_presets.len()
                    )
                } else {
                    "No layouts or devices could be recovered. Restored the default settings and layout.".to_string()
                };
                repair = Some(format!(
                    "The settings file was damaged. {kept} The damaged copy was saved to {}.",
                    backup.display()
                ));
                config
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(e.into()),
    };
    config.normalize();
    save(data_dir, &config)?;
    Ok((config, repair))
}

/// Keep complete members/elements at each depth, then close only their containers.
/// A scalar at EOF needs a delimiter to prove it wasn't cut in the middle.
fn close_truncated_json(bytes: &[u8]) -> Option<Value> {
    let bytes = match std::str::from_utf8(bytes) {
        Ok(_) => bytes,
        // A cut multibyte character at EOF must not discard earlier records.
        Err(error) if error.error_len().is_none() => &bytes[..error.valid_up_to()],
        Err(_) => return None,
    };
    #[derive(Clone, Copy)]
    enum Expect {
        FirstKey,
        Key,
        Colon,
        Value,
        FirstElement,
        Element,
        Comma,
    }
    struct Frame {
        closing: u8,
        complete_end: usize,
        expect: Expect,
    }
    let mut stack: Vec<Frame> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        if ch.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let expect = stack.last().map(|f| f.expect);
        match ch {
            b'{' | b'[' => {
                if !matches!(expect, None | Some(Expect::Value | Expect::FirstElement | Expect::Element)) {
                    return None;
                }
                stack.push(Frame {
                    closing: if ch == b'{' { b'}' } else { b']' },
                    complete_end: i + 1,
                    expect: if ch == b'{' { Expect::FirstKey } else { Expect::FirstElement },
                });
                i += 1;
            }
            b'}' | b']' => {
                let frame = stack.pop()?;
                if frame.closing != ch
                    || !matches!(frame.expect, Expect::FirstKey | Expect::FirstElement | Expect::Comma)
                {
                    return None;
                }
                i += 1;
                if let Some(parent) = stack.last_mut() {
                    parent.complete_end = i;
                    parent.expect = Expect::Comma;
                } else {
                    return serde_json::from_slice(bytes).ok();
                }
            }
            b':' if matches!(expect, Some(Expect::Colon)) => {
                stack.last_mut()?.expect = Expect::Value;
                i += 1;
            }
            b',' if matches!(expect, Some(Expect::Comma)) => {
                let frame = stack.last_mut()?;
                frame.expect = if frame.closing == b'}' { Expect::Key } else { Expect::Element };
                i += 1;
            }
            _ => {
                let key = matches!(expect, Some(Expect::FirstKey | Expect::Key));
                if !key && !matches!(expect, Some(Expect::Value | Expect::FirstElement | Expect::Element)) {
                    return None;
                }
                let start = i;
                if ch == b'"' {
                    i += 1;
                    let mut escaped = false;
                    while i < bytes.len() {
                        let ch = bytes[i];
                        i += 1;
                        if escaped {
                            escaped = false;
                        } else if ch == b'\\' {
                            escaped = true;
                        } else if ch == b'"' {
                            break;
                        }
                    }
                    // An escaped final quote does not finish a string.
                    if bytes.get(i - 1) != Some(&b'"') || escaped || i == start + 1 {
                        break;
                    }
                    // Checking the token also rejects malformed escapes and control characters.
                    if serde_json::from_slice::<String>(&bytes[start..i]).is_err() {
                        if i == bytes.len() {
                            break;
                        }
                        return None;
                    }
                } else {
                    if key {
                        return None;
                    }
                    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b',' | b'}' | b']')
                    {
                        i += 1;
                    }
                    if i == bytes.len() {
                        break;
                    }
                    serde_json::from_slice::<Value>(&bytes[start..i]).ok()?;
                }
                let frame = stack.last_mut()?;
                if key {
                    frame.expect = Expect::Colon;
                } else {
                    frame.complete_end = i;
                    frame.expect = Expect::Comma;
                }
            }
        }
    }
    let end = stack.last()?.complete_end;
    let mut repaired = bytes[..end].to_vec();
    for frame in stack.iter().rev() {
        repaired.push(frame.closing);
    }
    serde_json::from_slice(&repaired).ok()
}

/// The part of a config `value` that parses. `wrap` embeds a value at its
/// path in an otherwise empty config, so each part is checked on its own: a
/// bad list entry is dropped, and a bad field of a record whose fields all
/// have defaults falls back to its default.
fn salvage(mut value: Value, wrap: &dyn Fn(Value) -> Value) -> Option<Value> {
    if let Some(ai) = value.pointer_mut("/settings/ai").and_then(Value::as_object_mut)
        && ai
            .get("provider")
            .is_some_and(|provider| serde_json::from_value::<crate::AiProvider>(provider.clone()).is_err())
    {
        // Resetting the provider must never carry its key to the default provider.
        ai.remove("apiKey");
    }
    let parses = |value: &Value| serde_json::from_value::<Config>(wrap(value.clone())).is_ok();
    if parses(&value) {
        return Some(value);
    }
    let value = match value {
        Value::Array(items) => {
            Value::Array(items.into_iter().filter_map(|item| salvage(item, &|v| wrap(Value::Array(vec![v])))).collect())
        }
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter_map(|(key, field)| {
                    let field = salvage(field, &|v| wrap(Value::Object(Map::from_iter([(key.clone(), v)]))))?;
                    Some((key, field))
                })
                .collect(),
        ),
        _ => return None,
    };
    parses(&value).then_some(value)
}

/// Write a fresh temp file then rename it over the config, so a crash
/// mid-write can't truncate the config and concurrent saves can't collide.
pub fn save(data_dir: &Path, config: &Config) -> Result<(), StoreError> {
    fs::create_dir_all(data_dir)?;
    let path = config_path(data_dir);
    let tmp = path.with_extension(format!("json.{}.tmp", crate::new_id()));
    let bytes = serde_json::to_vec_pretty(config)?;
    write_private(&tmp, &bytes).and_then(|()| fs::rename(&tmp, &path)).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Create `path` readable by the owner only: the config holds API keys and tokens.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omni-core-{name}-{}", crate::new_id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn roundtrip_and_recover_from_corruption() {
        let dir = temp_dir("store");
        let first = load(&dir).unwrap();
        assert_eq!(first.profiles.len(), 1);
        assert_eq!(load(&dir).unwrap(), first);

        fs::write(config_path(&dir), b"{ not json").unwrap();
        let recovered = load(&dir).unwrap();
        assert_eq!(recovered.profiles.len(), 1);
        let broken = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().contains("broken"));
        assert!(broken);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn bad_records_cost_only_themselves() {
        let dir = temp_dir("salvage");
        let config = serde_json::json!({
            "settings": { "port": 7711, "voice": { "inputGainDb": "loud" } },
            "profiles": [{ "id": "mine", "name": "Mine", "pages": [{ "id": "main", "controls": [
                { "id": "ok", "x": 0, "y": 0, "kind": { "type": "button" } },
                { "id": "newer", "x": 1, "y": 0, "kind": { "type": "button", "action": { "type": "teleport" } } },
            ]}]}],
            "devices": [{ "id": "phone", "tokenHash": "abc", "deck": { "dim": "dark" } }],
            "themes": [{ "id": "custom-x", "name": "Partial", "colors": { "accent": "#ff0000" } }],
            "voicePresets": [{ "id": "v", "effects": [{ "type": "gain", "db": 3.0 }, { "type": "warp" }] }],
        });
        fs::write(config_path(&dir), serde_json::to_vec(&config).unwrap()).unwrap();

        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.settings.port, 7711);
        let profile = loaded.profile("mine").unwrap();
        let ids: Vec<_> = profile.controls().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["ok", "newer"]);
        assert!(matches!(profile.controls().nth(1).unwrap().kind, crate::ControlKind::Button { action: None }));
        assert_eq!(loaded.device("phone").unwrap().token_hash, "abc");
        let theme = &loaded.themes[0];
        assert_eq!((theme.colors.accent.as_str(), theme.radius), ("#ff0000", crate::defaults::default_theme().radius));
        assert_eq!(loaded.voice_presets[0].effects.len(), 1);
        assert!(fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains("broken")));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn salvaging_an_unknown_ai_provider_clears_its_key() {
        let dir = temp_dir("salvage-ai");
        for (provider, expected_provider, expected_key) in
            [("unknown", crate::AiProvider::Openai, ""), ("anthropic", crate::AiProvider::Anthropic, "test-key")]
        {
            let config = serde_json::json!({
                "settings": {
                    "port": "invalid",
                    "ai": { "provider": provider, "apiKey": "test-key" },
                },
            });
            fs::write(config_path(&dir), serde_json::to_vec(&config).unwrap()).unwrap();
            let (loaded, repaired) = load_with_repair_status(&dir).unwrap();
            assert!(repaired);
            assert_eq!(loaded.settings.ai.provider, expected_provider);
            assert_eq!(loaded.settings.ai.api_key, expected_key);
            let (reloaded, repaired) = load_with_repair_status(&dir).unwrap();
            assert!(!repaired);
            assert_eq!(reloaded, loaded);
        }
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_closing_braces_keep_layouts_and_devices() {
        let dir = temp_dir("truncated");
        let mut config = Config::default();
        config.profiles[0].name = "My layout".into();
        config.devices.push(crate::Device {
            id: "tablet".into(),
            name: "Tablet".into(),
            token_hash: "keep".into(),
            ..Default::default()
        });
        let mut bytes = serde_json::to_vec(&config).unwrap();
        bytes.pop();
        fs::write(config_path(&dir), &bytes).unwrap();
        let (loaded, warning) = load_with_repair_report(&dir).unwrap();
        assert_eq!(loaded, config);
        let warning = warning.unwrap();
        assert!(warning.contains("settings file was damaged"));
        assert!(warning.contains("devices: 1"));
        assert!(warning.contains("config.json.broken-"));
        assert!(fs::read_dir(&dir).unwrap().any(|entry| fs::read(entry.unwrap().path()).unwrap() == bytes));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn truncation_inside_utf8_keeps_preceding_layout_and_device() {
        let dir = temp_dir("truncated-utf8");
        let mut bytes = r#"{"profiles":[{"id":"mine","name":"Keep"}],"devices":[{"id":"tablet","tokenHash":"keep"}],"settings":{"hostName":"🎛"#
            .as_bytes()
            .to_vec();
        bytes.truncate(bytes.len() - 3); // Only the emoji's first byte reached disk.
        let error = std::str::from_utf8(&bytes).unwrap_err();
        assert_eq!(error.error_len(), None);
        fs::write(config_path(&dir), &bytes).unwrap();

        let (loaded, warning) = load_with_repair_report(&dir).unwrap();
        assert_eq!(loaded.profile("mine").unwrap().name, "Keep");
        assert_eq!(loaded.device("tablet").unwrap().token_hash, "keep");
        assert!(warning.is_some());
        assert_eq!(load_with_repair_report(&dir).unwrap(), (loaded, None));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn truncation_discards_unfinished_values_and_keeps_complete_records() {
        let prefix = r#"{"profiles":[{"id":"mine","name":"Keep"}],"settings":{"hostName":"Desk","ai":{"model":"kept","#;
        for cut in [r#""apiKey":"test-"#, r#""baseUrl":"http://127.0."#, r#""apiK"#, r#""apiKey":"#] {
            let repaired = close_truncated_json(format!("{prefix}{cut}").as_bytes()).unwrap();
            assert_eq!(repaired["profiles"][0]["id"], "mine");
            assert_eq!(repaired["settings"]["ai"], serde_json::json!({"model":"kept"}));
        }
        for cut in [r#", "port":77"#, r#", "startHidden":tru"#, ",", r#", "port":"#, r#", "por"#] {
            let text = format!(r#"{{"profiles":[{{"id":"mine"}}],"settings":{{"hostName":"Desk"{cut}"#);
            let repaired = close_truncated_json(text.as_bytes()).unwrap();
            assert_eq!(repaired["profiles"][0]["id"], "mine");
            assert_eq!(repaired["settings"], serde_json::json!({"hostName":"Desk"}));
        }
        assert_eq!(close_truncated_json(br#"{"a":[1,2"#), Some(serde_json::json!({"a":[1]})));
        assert_eq!(close_truncated_json(br#"{"a":[true,"#), Some(serde_json::json!({"a":[true]})));
        assert_eq!(
            close_truncated_json(br#"{"a":[{"id":"keep"},{"id":"cut"#),
            Some(serde_json::json!({"a":[{"id":"keep"},{}]}))
        );
    }

    #[test]
    fn repair_respects_string_escapes_and_handles_empty_or_non_utf8_input() {
        let value = serde_json::json!({"a": [r#"braces } ] and "quoted" and \slash"#]});
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.truncate(bytes.len() - 2);
        assert_eq!(close_truncated_json(&bytes), Some(value));
        for suffix in [r#""unfinished\"#, r#""unfinished\"quote"#] {
            let text = format!(r#"{{"kept":"yes","cut":{suffix}"#);
            assert_eq!(close_truncated_json(text.as_bytes()), Some(serde_json::json!({"kept":"yes"})));
        }
        assert!(close_truncated_json(br#"{"a":]"#).is_none());
        for bytes in [b"".as_slice(), b"\xff", b"{not json"] {
            assert!(close_truncated_json(bytes).is_none());
            let dir = temp_dir("invalid-bytes");
            fs::write(config_path(&dir), bytes).unwrap();
            let (_, warning) = load_with_repair_report(&dir).unwrap();
            assert!(warning.is_some());
            fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn concurrent_saves_dont_collide() {
        let dir = temp_dir("concurrent");
        let config = Config::default();
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| (0..20).for_each(|_| save(&dir, &config).unwrap()));
            }
        });
        let files: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(files, [CONFIG_FILE]);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn data_dir_lock_is_exclusive() {
        let dir = temp_dir("lock");
        let held = lock(&dir).unwrap().unwrap();
        assert!(lock(&dir).unwrap().is_none());
        drop(held);
        assert!(lock(&dir).unwrap().is_some());
        fs::remove_dir_all(dir).ok();
    }
}
