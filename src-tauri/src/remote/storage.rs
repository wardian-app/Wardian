use crate::remote::models::{RemoteGatewayConfig, REMOTE_SETTINGS_SCHEMA_VERSION};
use std::path::{Path, PathBuf};

const REMOTE_CONFIG_FILE: &str = "remote-access/config.json";

pub fn remote_config_path(wardian_home: &Path) -> PathBuf {
    wardian_home.join(REMOTE_CONFIG_FILE)
}

pub fn load_remote_config() -> Result<Option<RemoteGatewayConfig>, String> {
    let home = crate::utils::get_wardian_home().ok_or("Could not find Wardian home")?;
    load_remote_config_at(&home)
}

pub fn save_remote_config(config: &RemoteGatewayConfig) -> Result<RemoteGatewayConfig, String> {
    save_remote_config_at(
        &crate::utils::get_wardian_home().ok_or("Could not find Wardian home")?,
        config,
    )?;
    Ok(config.clone())
}

pub fn load_remote_config_at(home: &Path) -> Result<Option<RemoteGatewayConfig>, String> {
    let path = remote_config_path(home);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let config =
        serde_json::from_str::<RemoteGatewayConfig>(&content).map_err(|error| error.to_string())?;
    if config.schema_version != REMOTE_SETTINGS_SCHEMA_VERSION {
        return Err(format!(
            "unsupported remote access schema {}",
            config.schema_version
        ));
    }
    Ok(Some(config))
}

pub fn save_remote_config_at(home: &Path, config: &RemoteGatewayConfig) -> Result<(), String> {
    let path = remote_config_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let content = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    std::fs::write(path, content).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::models::{RemoteGatewayConfig, REMOTE_SETTINGS_SCHEMA_VERSION};

    #[test]
    fn remote_config_path_lives_under_wardian_home() {
        let path = remote_config_path(std::path::Path::new("/tmp/wardian-home"));
        assert_eq!(
            path.to_string_lossy().replace('\\', "/"),
            "/tmp/wardian-home/remote-access/config.json"
        );
    }

    #[test]
    fn disabled_config_roundtrips() {
        let temp = tempfile::tempdir().expect("temp dir");
        let config = RemoteGatewayConfig {
            schema_version: REMOTE_SETTINGS_SCHEMA_VERSION,
            enabled: false,
            canonical_origin: String::new(),
            loopback_host: "127.0.0.1".to_string(),
            loopback_port: 0,
            gateway_identity_public_key: "pub".to_string(),
            gateway_identity_fingerprint: "fp".to_string(),
        };

        save_remote_config_at(temp.path(), &config).expect("save config");
        let loaded = load_remote_config_at(temp.path()).expect("load config");

        assert_eq!(loaded, Some(config));
    }
}
