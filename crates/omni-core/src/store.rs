//! Loading and saving `config.json` atomically.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::model::Config;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("config io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config is not valid json: {0}")]
    Json(#[from] serde_json::Error),
}

pub const CONFIG_FILE: &str = "config.json";

pub fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONFIG_FILE)
}

/// Load the config, creating a default one when missing. A corrupt file is
/// kept as `config.json.broken-<ms>` and replaced by defaults, so a bad edit
/// never prevents the app from starting.
pub fn load(data_dir: &Path) -> Result<Config, StoreError> {
    let path = config_path(data_dir);
    let mut config = match fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Config>(&bytes) {
            Ok(config) => config,
            Err(_) => {
                let backup = path.with_extension(format!("json.broken-{}", crate::now_ms()));
                fs::rename(&path, backup)?;
                Config::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(e.into()),
    };
    config.normalize();
    save(data_dir, &config)?;
    Ok(config)
}

/// Write to a temp file then rename, so a crash mid-write can't truncate the config.
pub fn save(data_dir: &Path, config: &Config) -> Result<(), StoreError> {
    fs::create_dir_all(data_dir)?;
    let path = config_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(config)?;
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    restrict_permissions(&tmp);
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// The config holds API keys and tokens: keep it readable by the owner only.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
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
}
