use crate::remote::models::{
    DeviceCredentialKind, DeviceRecord, RemoteCredentialAudience, RemoteDeviceStore,
    RemoteGatewayConfig, REMOTE_DEVICE_STORE_SCHEMA_VERSION, REMOTE_SETTINGS_SCHEMA_VERSION,
};
use fs2::FileExt;
use serde::Deserialize;
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

#[derive(Deserialize)]
struct LegacyRemoteDeviceStore {
    devices: Vec<LegacyDeviceRecord>,
}

#[derive(Deserialize)]
struct LegacyDeviceRecord {
    device_id: String,
    label: String,
    public_key_spki_der_base64: String,
    public_key_fingerprint: String,
    created_at: String,
    last_used_at: Option<String>,
    revoked_at: Option<String>,
}

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
    let document =
        serde_json::from_str::<serde_json::Value>(&content).map_err(|error| error.to_string())?;
    let schema_version = document
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u8::try_from(version).ok())
        .ok_or_else(|| "remote device store schema version is invalid".to_string())?;
    let store = match schema_version {
        1 => migrate_v1_device_store(
            serde_json::from_value::<LegacyRemoteDeviceStore>(document)
                .map_err(|error| error.to_string())?,
        ),
        REMOTE_DEVICE_STORE_SCHEMA_VERSION => serde_json::from_value::<RemoteDeviceStore>(document)
            .map_err(|error| error.to_string())?,
        version => return Err(format!("unsupported remote device store schema {version}")),
    };
    validate_device_store(&store)?;
    Ok(store)
}

fn migrate_v1_device_store(legacy: LegacyRemoteDeviceStore) -> RemoteDeviceStore {
    RemoteDeviceStore {
        schema_version: REMOTE_DEVICE_STORE_SCHEMA_VERSION,
        devices: legacy
            .devices
            .into_iter()
            .map(|device| DeviceRecord {
                device_id: device.device_id,
                label: device.label,
                public_key_spki_der_base64: device.public_key_spki_der_base64,
                public_key_fingerprint: device.public_key_fingerprint,
                credential_kind: DeviceCredentialKind::Pwa,
                audience: RemoteCredentialAudience::Pwa,
                client_origin: None,
                read_grants: Vec::new(),
                send_grants: Vec::new(),
                created_at: device.created_at,
                last_used_at: device.last_used_at,
                revoked_at: device.revoked_at,
            })
            .collect(),
    }
}

fn validate_device_store(store: &RemoteDeviceStore) -> Result<(), String> {
    if store.schema_version != REMOTE_DEVICE_STORE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported remote device store schema {}",
            store.schema_version
        ));
    }
    let mut ids = HashSet::with_capacity(store.devices.len());
    for device in &store.devices {
        if !ids.insert(device.device_id.as_str()) {
            return Err("duplicate_remote_device_id".to_string());
        }
        let valid_authority = match device.credential_kind {
            DeviceCredentialKind::Pwa => {
                device.audience == RemoteCredentialAudience::Pwa
                    && device.client_origin.is_none()
                    && device.read_grants.is_empty()
                    && device.send_grants.is_empty()
            }
            DeviceCredentialKind::PairedClient => {
                device.audience == RemoteCredentialAudience::PairedClientV1
                    && device.client_origin.as_deref().is_some_and(|origin| {
                        crate::remote::policy::parse_client_origin(origin)
                            .is_ok_and(|parsed| parsed.raw() == origin)
                    })
                    && !device
                        .read_grants
                        .iter()
                        .enumerate()
                        .any(|(index, grant)| device.read_grants[..index].contains(grant))
                    && !device
                        .send_grants
                        .iter()
                        .enumerate()
                        .any(|(index, grant)| device.send_grants[..index].contains(grant))
            }
        };
        if !valid_authority {
            return Err("remote_device_authority_invalid".to_string());
        }
    }
    Ok(())
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
    validate_device_store(updated)?;

    for previous in &original.devices {
        let current = updated
            .devices
            .iter()
            .find(|device| device.device_id == previous.device_id)
            .ok_or_else(|| "remote_device_store_records_cannot_be_removed".to_string())?;
        if current.label != previous.label
            || current.public_key_spki_der_base64 != previous.public_key_spki_der_base64
            || current.public_key_fingerprint != previous.public_key_fingerprint
            || current.credential_kind != previous.credential_kind
            || current.audience != previous.audience
            || current.client_origin != previous.client_origin
            || current.read_grants != previous.read_grants
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
        if let (Some(previous), Some(current)) = (
            previous.last_used_at.as_deref(),
            current.last_used_at.as_deref(),
        ) {
            let previous_ms = chrono::DateTime::parse_from_rfc3339(previous)
                .map_err(|_| "remote_device_last_used_invalid".to_string())?
                .timestamp_millis();
            let current_ms = chrono::DateTime::parse_from_rfc3339(current)
                .map_err(|_| "remote_device_last_used_invalid".to_string())?
                .timestamp_millis();
            if current_ms < previous_ms {
                return Err("remote_device_last_used_cannot_move_backwards".to_string());
            }
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
    advance_device_last_used(device, last_used_at)?;
    Ok(())
}

pub(crate) fn advance_device_last_used(
    device: &mut DeviceRecord,
    last_used_at: &str,
) -> Result<(), String> {
    let submitted_ms = chrono::DateTime::parse_from_rfc3339(last_used_at)
        .map_err(|_| "remote_device_last_used_invalid".to_string())?
        .timestamp_millis();
    let current_ms = device
        .last_used_at
        .as_deref()
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|_| "remote_device_last_used_invalid".to_string())?
        .map(|value| value.timestamp_millis());
    if current_ms.is_none_or(|current_ms| submitted_ms > current_ms) {
        device.last_used_at = Some(last_used_at.to_string());
    }
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
        DeviceCredentialKind, DeviceRecord, RemoteCredentialAudience, RemoteGatewayConfig,
        RemoteReadGrant, REMOTE_SETTINGS_SCHEMA_VERSION,
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
            client_origin_allowlist: Vec::new(),
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
    fn legacy_remote_config_without_client_allowlist_disables_client_origins() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = remote_config_path(temp.path());
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"enabled":false,"canonical_origin":"","loopback_host":"127.0.0.1","loopback_port":0,"gateway_identity_public_key":"pub","gateway_identity_fingerprint":"fp"}"#,
        )
        .expect("write legacy config");
        let loaded = load_remote_config_at(temp.path())
            .expect("load legacy config")
            .expect("config exists");
        assert!(loaded.client_origin_allowlist.is_empty());
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
            credential_kind: DeviceCredentialKind::Pwa,
            audience: RemoteCredentialAudience::Pwa,
            client_origin: None,
            read_grants: Vec::new(),
            send_grants: Vec::new(),
            created_at: "2026-05-21T00:00:00Z".to_string(),
            last_used_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn schema_one_device_store_migrates_to_pwa_and_publishes_schema_two_on_mutation() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = remote_devices_path(temp.path());
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"devices":[{"device_id":"dev-1","label":"Phone","public_key_spki_der_base64":"cHVi","public_key_fingerprint":"fp","created_at":"2026-05-21T00:00:00Z","last_used_at":null,"revoked_at":null}]}"#,
        )
        .expect("write schema one store");

        let migrated = load_device_store_at(temp.path()).expect("migrate legacy store in memory");
        assert_eq!(migrated.schema_version, REMOTE_DEVICE_STORE_SCHEMA_VERSION);
        assert_eq!(
            migrated.devices[0].credential_kind,
            DeviceCredentialKind::Pwa
        );
        assert_eq!(migrated.devices[0].audience, RemoteCredentialAudience::Pwa);
        assert_eq!(migrated.devices[0].client_origin, None);
        assert!(migrated.devices[0].read_grants.is_empty());
        let before_publication = serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(&path).expect("read original legacy bytes"),
        )
        .expect("parse original");
        assert_eq!(before_publication["schema_version"], 1);

        update_device_store_at(temp.path(), |store| {
            advance_device_last_used(
                store.devices.first_mut().expect("migrated device"),
                "2026-05-21T00:01:00Z",
            )?;
            Ok(())
        })
        .expect("publish migration through latest-store transaction");
        let published = serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(&path).expect("read migrated bytes"),
        )
        .expect("parse migrated store");
        assert_eq!(
            published["schema_version"],
            REMOTE_DEVICE_STORE_SCHEMA_VERSION
        );
        assert_eq!(published["devices"][0]["credential_kind"], "pwa");
        assert_eq!(published["devices"][0]["audience"], "wardian_remote_pwa");
        assert!(published["devices"][0]["client_origin"].is_null());
        assert_eq!(
            published["devices"][0]["read_grants"],
            serde_json::json!([])
        );
    }

    #[test]
    fn schema_two_rejects_missing_or_inconsistent_authority_fields() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = remote_devices_path(temp.path());
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        std::fs::write(
            &path,
            r#"{"schema_version":2,"devices":[{"device_id":"dev-1","label":"Phone","public_key_spki_der_base64":"cHVi","public_key_fingerprint":"fp","credential_kind":"pwa","audience":"wardian_remote_pwa","read_grants":[],"created_at":"2026-05-21T00:00:00Z","last_used_at":null,"revoked_at":null}]}"#,
        )
        .expect("write malformed schema two store");
        assert!(load_device_store_at(temp.path()).is_err());

        std::fs::write(
            &path,
            r#"{"schema_version":2,"devices":[{"device_id":"dev-1","label":"Phone","public_key_spki_der_base64":"cHVi","public_key_fingerprint":"fp","credential_kind":"pwa","audience":"wardian_remote_client_v1","client_origin":null,"read_grants":[],"created_at":"2026-05-21T00:00:00Z","last_used_at":null,"revoked_at":null}]}"#,
        )
        .expect("write inconsistent schema two store");
        assert_eq!(
            load_device_store_at(temp.path()).expect_err("inconsistent authority rejected"),
            "remote_device_authority_invalid"
        );
    }

    #[test]
    fn credential_authority_is_immutable_across_device_store_transitions() {
        let original = RemoteDeviceStore {
            schema_version: REMOTE_DEVICE_STORE_SCHEMA_VERSION,
            devices: vec![device("dev-1", "Phone", "fp")],
        };
        let mut updated = original.clone();
        let record = &mut updated.devices[0];
        record.credential_kind = DeviceCredentialKind::PairedClient;
        record.audience = RemoteCredentialAudience::PairedClientV1;
        record.client_origin = Some("https://client.example".to_string());
        record.read_grants = vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead];
        assert_eq!(
            validate_device_store_transition(&original, &updated)
                .expect_err("authority changes require a new identity"),
            "remote_device_identity_cannot_be_changed"
        );
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
