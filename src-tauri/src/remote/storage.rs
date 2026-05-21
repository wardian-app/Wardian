use crate::remote::models::{
    RemoteDeviceStore, RemoteGatewayConfig, REMOTE_DEVICE_STORE_SCHEMA_VERSION,
    REMOTE_SETTINGS_SCHEMA_VERSION,
};
use std::path::{Path, PathBuf};

const REMOTE_CONFIG_FILE: &str = "remote-access/config.json";
const REMOTE_DEVICES_FILE: &str = "remote-access/devices.json";

pub fn remote_config_path(wardian_home: &Path) -> PathBuf {
    wardian_home.join(REMOTE_CONFIG_FILE)
}

pub fn remote_devices_path(wardian_home: &Path) -> PathBuf {
    wardian_home.join(REMOTE_DEVICES_FILE)
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

pub fn load_device_store() -> Result<RemoteDeviceStore, String> {
    let home = crate::utils::get_wardian_home().ok_or("Could not find Wardian home")?;
    load_device_store_at(&home)
}

pub fn save_device_store(store: &RemoteDeviceStore) -> Result<RemoteDeviceStore, String> {
    let home = crate::utils::get_wardian_home().ok_or("Could not find Wardian home")?;
    save_device_store_at(&home, store)?;
    Ok(store.clone())
}

pub fn load_device_store_at(home: &Path) -> Result<RemoteDeviceStore, String> {
    let path = remote_devices_path(home);
    if !path.exists() {
        return Ok(RemoteDeviceStore {
            schema_version: REMOTE_DEVICE_STORE_SCHEMA_VERSION,
            devices: Vec::new(),
        });
    }
    let content = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let store =
        serde_json::from_str::<RemoteDeviceStore>(&content).map_err(|error| error.to_string())?;
    if store.schema_version != REMOTE_DEVICE_STORE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported remote device store schema {}",
            store.schema_version
        ));
    }
    Ok(store)
}

pub fn save_device_store_at(home: &Path, store: &RemoteDeviceStore) -> Result<(), String> {
    if store.schema_version != REMOTE_DEVICE_STORE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported remote device store schema {}",
            store.schema_version
        ));
    }
    let path = remote_devices_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let content = serde_json::to_string_pretty(store).map_err(|error| error.to_string())?;
    std::fs::write(path, content).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::models::{
        DeviceRecord, RemoteDeviceStore, RemoteGatewayConfig, REMOTE_DEVICE_STORE_SCHEMA_VERSION,
        REMOTE_SETTINGS_SCHEMA_VERSION,
    };

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

    #[test]
    fn device_store_path_lives_under_wardian_home() {
        let path = remote_devices_path(std::path::Path::new("/tmp/wardian-home"));
        assert_eq!(
            path.to_string_lossy().replace('\\', "/"),
            "/tmp/wardian-home/remote-access/devices.json"
        );
    }

    #[test]
    fn device_store_roundtrips_and_revokes() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut devices = RemoteDeviceStore {
            schema_version: REMOTE_DEVICE_STORE_SCHEMA_VERSION,
            devices: vec![DeviceRecord {
                device_id: "dev-1".to_string(),
                label: "Phone".to_string(),
                public_key_spki_der_base64: "key".to_string(),
                public_key_fingerprint: "fp".to_string(),
                created_at: "2026-05-21T00:00:00Z".to_string(),
                last_used_at: None,
                revoked_at: None,
            }],
        };

        save_device_store_at(temp.path(), &devices).expect("save devices");
        let loaded = load_device_store_at(temp.path()).expect("load devices");

        assert_eq!(loaded.devices[0].device_id, "dev-1");
        assert_eq!(loaded.devices[0].revoked_at, None);

        devices.devices[0].revoked_at = Some("2026-05-21T00:05:00Z".to_string());
        save_device_store_at(temp.path(), &devices).expect("save revoked devices");
        let revoked = load_device_store_at(temp.path()).expect("load revoked devices");

        assert_eq!(
            revoked.devices[0].revoked_at.as_deref(),
            Some("2026-05-21T00:05:00Z")
        );
    }
}
