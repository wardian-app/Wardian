use crate::remote::models::{
    RemoteDeviceStore, RemoteGatewayConfig, REMOTE_DEVICE_STORE_SCHEMA_VERSION,
    REMOTE_SETTINGS_SCHEMA_VERSION,
};
use fs2::FileExt;
use std::{
    collections::HashSet,
    fmt,
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const REMOTE_CONFIG_FILE: &str = "remote-access/config.json";
const REMOTE_DEVICES_FILE: &str = "remote-access/devices.json";
const REMOTE_DEVICES_LOCK_FILE: &str = "remote-access/devices.lock";

// Keep local mutation threads ordered; the adjacent OS lock covers other processes.
static REMOTE_DEVICE_STORE_PROCESS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceStorePublishState {
    NotPublished,
    Published,
    Unknown,
}

#[derive(Debug)]
pub(crate) struct DeviceStoreUpdateError {
    pub(crate) message: String,
    pub(crate) publish_state: DeviceStorePublishState,
}

impl fmt::Display for DeviceStoreUpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

pub fn remote_config_path(wardian_home: &Path) -> PathBuf {
    wardian_home.join(REMOTE_CONFIG_FILE)
}

pub fn remote_devices_path(wardian_home: &Path) -> PathBuf {
    wardian_home.join(REMOTE_DEVICES_FILE)
}

fn remote_devices_lock_path(wardian_home: &Path) -> PathBuf {
    wardian_home.join(REMOTE_DEVICES_LOCK_FILE)
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

/// Applies a field-level mutation to the latest store on the blocking pool.
pub(crate) async fn update_device_store<T, F>(
    update: F,
) -> Result<(RemoteDeviceStore, T), DeviceStoreUpdateError>
where
    T: Send + 'static,
    F: FnOnce(&mut RemoteDeviceStore) -> Result<T, String> + Send + 'static,
{
    let home = crate::utils::get_wardian_home().ok_or_else(|| {
        device_store_update_error(
            "Could not find Wardian home".to_string(),
            DeviceStorePublishState::NotPublished,
        )
    })?;
    tokio::task::spawn_blocking(move || update_device_store_at(&home, update))
        .await
        .map_err(|error| DeviceStoreUpdateError {
            message: error.to_string(),
            publish_state: DeviceStorePublishState::Unknown,
        })?
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

/// Serializes read-modify-write across local threads and separate processes.
pub(crate) fn update_device_store_at<T>(
    home: &Path,
    update: impl FnOnce(&mut RemoteDeviceStore) -> Result<T, String>,
) -> Result<(RemoteDeviceStore, T), DeviceStoreUpdateError> {
    update_device_store_at_with_writer(home, update, |path, store| {
        wardian_core::conversations::write_json_atomic(path, store)
            .map_err(|error| error.to_string())
    })
}

fn update_device_store_at_with_writer<T>(
    home: &Path,
    update: impl FnOnce(&mut RemoteDeviceStore) -> Result<T, String>,
    publish: impl FnOnce(&Path, &RemoteDeviceStore) -> Result<(), String>,
) -> Result<(RemoteDeviceStore, T), DeviceStoreUpdateError> {
    let process_lock = REMOTE_DEVICE_STORE_PROCESS_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = remote_devices_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            device_store_update_error(error.to_string(), DeviceStorePublishState::NotPublished)
        })?;
    }
    let lock_path = remote_devices_lock_path(home);
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| {
            device_store_update_error(error.to_string(), DeviceStorePublishState::NotPublished)
        })?;
    FileExt::lock_exclusive(&lock_file).map_err(|error| {
        device_store_update_error(error.to_string(), DeviceStorePublishState::NotPublished)
    })?;

    let original = load_device_store_at(home)
        .map_err(|error| device_store_update_error(error, DeviceStorePublishState::NotPublished))?;
    let mut updated = original.clone();
    let result = update(&mut updated)
        .map_err(|error| device_store_update_error(error, DeviceStorePublishState::NotPublished))?;
    validate_device_store_transition(&original, &updated)
        .map_err(|error| device_store_update_error(error, DeviceStorePublishState::NotPublished))?;
    if let Err(error) = publish(&path, &updated) {
        let publish_state = match load_device_store_at(home) {
            Ok(current) if current == updated => DeviceStorePublishState::Published,
            Ok(current) if current == original => DeviceStorePublishState::NotPublished,
            _ => DeviceStorePublishState::Unknown,
        };
        return Err(device_store_update_error(error, publish_state));
    }

    drop(lock_file);
    drop(process_lock);
    Ok((updated, result))
}

fn device_store_update_error(
    message: String,
    publish_state: DeviceStorePublishState,
) -> DeviceStoreUpdateError {
    DeviceStoreUpdateError {
        message,
        publish_state,
    }
}

fn validate_device_store_transition(
    original: &RemoteDeviceStore,
    updated: &RemoteDeviceStore,
) -> Result<(), String> {
    if updated.schema_version != REMOTE_DEVICE_STORE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported remote device store schema {}",
            updated.schema_version
        ));
    }

    let mut ids = HashSet::with_capacity(updated.devices.len());
    for device in &updated.devices {
        if !ids.insert(device.device_id.as_str()) {
            return Err("duplicate_remote_device_id".to_string());
        }
    }

    for previous in &original.devices {
        let current = updated
            .devices
            .iter()
            .find(|device| device.device_id == previous.device_id)
            .ok_or_else(|| "remote_device_store_records_cannot_be_removed".to_string())?;
        if current.label != previous.label
            || current.public_key_spki_der_base64 != previous.public_key_spki_der_base64
            || current.public_key_fingerprint != previous.public_key_fingerprint
            || current.created_at != previous.created_at
        {
            return Err("remote_device_identity_cannot_be_changed".to_string());
        }
        if previous.revoked_at.is_some() && current.revoked_at != previous.revoked_at {
            return Err("remote_device_revocation_cannot_be_cleared_or_changed".to_string());
        }
        if previous.last_used_at.is_some() && current.last_used_at.is_none() {
            return Err("remote_device_last_used_cannot_be_cleared".to_string());
        }
    }
    Ok(())
}

fn record_device_last_used(
    store: &mut RemoteDeviceStore,
    device_id: &str,
    last_used_at: &str,
) -> Result<(), String> {
    let device = store
        .devices
        .iter_mut()
        .find(|device| device.device_id == device_id)
        .ok_or_else(|| "remote_device_not_found".to_string())?;
    if device.revoked_at.is_some() {
        return Err("remote_device_revoked".to_string());
    }
    device.last_used_at = Some(last_used_at.to_string());
    Ok(())
}

pub(crate) async fn update_device_last_used(
    device_id: String,
    last_used_at: String,
) -> Result<RemoteDeviceStore, DeviceStoreUpdateError> {
    let (store, ()) =
        update_device_store(move |store| record_device_last_used(store, &device_id, &last_used_at))
            .await?;
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::models::{
        DeviceRecord, RemoteGatewayConfig, REMOTE_SETTINGS_SCHEMA_VERSION,
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
        assert_eq!(
            remote_devices_lock_path(std::path::Path::new("/tmp/wardian-home"))
                .to_string_lossy()
                .replace('\\', "/"),
            "/tmp/wardian-home/remote-access/devices.lock"
        );
    }

    fn device(device_id: &str, label: &str, fingerprint: &str) -> DeviceRecord {
        DeviceRecord {
            device_id: device_id.to_string(),
            label: label.to_string(),
            public_key_spki_der_base64: format!("key-{device_id}"),
            public_key_fingerprint: fingerprint.to_string(),
            created_at: "2026-05-21T00:00:00Z".to_string(),
            last_used_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn device_store_roundtrips_and_revokes() {
        let temp = tempfile::tempdir().expect("temp dir");
        update_device_store_at(temp.path(), |store| {
            store.devices.push(device("dev-1", "Phone", "fp-1"));
            Ok(())
        })
        .expect("save initial device");
        let loaded = load_device_store_at(temp.path()).expect("load devices");

        assert_eq!(loaded.devices[0].device_id, "dev-1");
        assert_eq!(loaded.devices[0].revoked_at, None);

        update_device_store_at(temp.path(), |store| {
            store.devices[0].revoked_at = Some("2026-05-21T00:05:00Z".to_string());
            Ok(())
        })
        .expect("save revoked device");
        let revoked = load_device_store_at(temp.path()).expect("load revoked devices");

        assert_eq!(
            revoked.devices[0].revoked_at.as_deref(),
            Some("2026-05-21T00:05:00Z")
        );
    }

    #[test]
    fn delayed_last_used_write_cannot_restore_revoked_device_or_drop_other_devices() {
        let temp = tempfile::tempdir().expect("temp dir");
        update_device_store_at(temp.path(), |store| {
            store.devices.push(device("dev-1", "Phone", "fp-1"));
            store.devices.push(device("dev-2", "Tablet", "fp-2"));
            Ok(())
        })
        .expect("seed devices");

        let stale_authorization = load_device_store_at(temp.path())
            .expect("pre-revocation read")
            .devices
            .into_iter()
            .find(|device| device.device_id == "dev-1" && device.revoked_at.is_none())
            .expect("device active in stale read");

        update_device_store_at(temp.path(), |store| {
            let device = store
                .devices
                .iter_mut()
                .find(|device| device.device_id == "dev-1")
                .expect("device to revoke");
            device.revoked_at = Some("2026-05-21T00:05:00Z".to_string());
            Ok(())
        })
        .expect("revoke after stale authorization read");

        let delayed_write = update_device_store_at(temp.path(), |store| {
            record_device_last_used(
                store,
                &stale_authorization.device_id,
                "2026-05-21T00:06:00Z",
            )
        })
        .expect_err("late auth completion must recheck current authority");
        assert_eq!(delayed_write.message, "remote_device_revoked");
        assert_eq!(
            delayed_write.publish_state,
            DeviceStorePublishState::NotPublished
        );

        let current = load_device_store_at(temp.path()).expect("load current devices");
        assert_eq!(current.devices.len(), 2);
        assert_eq!(current.devices[0].last_used_at, None);
        assert_eq!(
            current.devices[0].revoked_at.as_deref(),
            Some("2026-05-21T00:05:00Z")
        );
        assert_eq!(current.devices[1], device("dev-2", "Tablet", "fp-2"));
    }

    #[test]
    fn device_store_transition_cannot_clear_an_existing_revocation() {
        let temp = tempfile::tempdir().expect("temp dir");
        update_device_store_at(temp.path(), |store| {
            store.devices.push(device("dev-1", "Phone", "fp-1"));
            Ok(())
        })
        .expect("seed device");
        update_device_store_at(temp.path(), |store| {
            store.devices[0].revoked_at = Some("2026-05-21T00:05:00Z".to_string());
            Ok(())
        })
        .expect("revoke device");

        let error = update_device_store_at(temp.path(), |store| {
            store.devices[0].revoked_at = None;
            Ok(())
        })
        .expect_err("revocation is irreversible through store mutations");

        assert_eq!(
            error.message,
            "remote_device_revocation_cannot_be_cleared_or_changed"
        );
        assert_eq!(
            load_device_store_at(temp.path())
                .expect("load still-revoked device")
                .devices[0]
                .revoked_at
                .as_deref(),
            Some("2026-05-21T00:05:00Z")
        );
    }

    #[test]
    fn failed_device_store_publication_preserves_previous_complete_store() {
        let temp = tempfile::tempdir().expect("temp dir");
        update_device_store_at(temp.path(), |store| {
            store.devices.push(device("dev-1", "Phone", "fp-1"));
            Ok(())
        })
        .expect("seed device");
        let original = load_device_store_at(temp.path()).expect("load original");

        let error = update_device_store_at_with_writer(
            temp.path(),
            |store| {
                store.devices[0].last_used_at = Some("2026-05-21T00:06:00Z".to_string());
                Ok(())
            },
            |_, _| Err("injected before publish".to_string()),
        )
        .expect_err("publisher failure must be returned");

        assert_eq!(error.publish_state, DeviceStorePublishState::NotPublished);
        assert_eq!(
            load_device_store_at(temp.path()).expect("load after failed publish"),
            original
        );
    }

    #[test]
    fn post_publish_error_is_reported_as_published() {
        let temp = tempfile::tempdir().expect("temp dir");
        update_device_store_at(temp.path(), |store| {
            store.devices.push(device("dev-1", "Phone", "fp-1"));
            Ok(())
        })
        .expect("seed device");

        let error = update_device_store_at_with_writer(
            temp.path(),
            |store| {
                store.devices[0].revoked_at = Some("2026-05-21T00:05:00Z".to_string());
                Ok(())
            },
            |path, store| {
                wardian_core::conversations::write_json_atomic(path, store)
                    .map_err(|error| error.to_string())?;
                Err("injected after publish".to_string())
            },
        )
        .expect_err("post-publish error must remain visible");

        assert_eq!(error.publish_state, DeviceStorePublishState::Published);
        assert_eq!(
            load_device_store_at(temp.path())
                .expect("load after post-publish error")
                .devices[0]
                .revoked_at
                .as_deref(),
            Some("2026-05-21T00:05:00Z")
        );
    }
}
