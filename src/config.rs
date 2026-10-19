// SPDX-License-Identifier: MPL-2.0

use crate::model::UnitId;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};

const MAX_BYTES: usize = 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinFile<T> {
    version: u8,
    pins: T,
}

fn path() -> Result<PathBuf, String> {
    let root = match env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => PathBuf::from(
            env::var_os("HOME").ok_or("HOME is unavailable; cannot locate saved pins.")?,
        )
        .join(".config"),
    };
    if !root.is_absolute() {
        return Err("The configuration directory must be an absolute path.".into());
    }
    Ok(root.join("cosmos-applet-systemd/pins.json"))
}

fn validate(pins: &[UnitId]) -> Result<(), String> {
    let mut seen = HashSet::with_capacity(pins.len());
    for id in pins {
        id.validate()?;
        if !seen.insert(id) {
            return Err("Saved pins contain a duplicate unit in the same scope.".into());
        }
    }
    Ok(())
}

fn decode(bytes: &[u8]) -> Result<Vec<UnitId>, String> {
    let file: PinFile<Vec<UnitId>> = serde_json::from_slice(bytes)
        .map_err(|error| format!("Saved pins contain invalid JSON: {error}"))?;
    if file.version != 1 {
        return Err(
            "Saved pins use an unsupported format version. The file has not been changed.".into(),
        );
    }
    validate(&file.pins)?;
    Ok(file.pins)
}

pub async fn load() -> Result<Vec<UnitId>, String> {
    load_from(&path()?).await
}

async fn load_from(path: &Path) -> Result<Vec<UnitId>, String> {
    let metadata = match fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Could not inspect saved pins: {error}")),
    };
    if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
        return Err("Saved pins must be a regular JSON file no larger than 1 MiB.".into());
    }
    let file = fs::File::open(path)
        .await
        .map_err(|error| format!("Could not open saved pins: {error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("Could not read saved pins: {error}"))?;
    if bytes.len() > MAX_BYTES {
        return Err("Saved pins exceed the 1 MiB limit.".into());
    }
    decode(&bytes)
}

pub async fn save(pins: &[UnitId]) -> Result<(), String> {
    save_to(&path()?, pins).await
}

async fn save_to(path: &Path, pins: &[UnitId]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    validate(pins)?;
    let bytes = serde_json::to_vec_pretty(&PinFile { version: 1, pins })
        .map_err(|error| format!("Could not encode pins: {error}"))?;
    if bytes.len() > MAX_BYTES {
        return Err("Pins exceed the 1 MiB configuration limit.".into());
    }
    let parent = path.parent().ok_or("Invalid pin configuration path.")?;
    fs::create_dir_all(parent)
        .await
        .map_err(|error| format!("Could not create pin configuration directory: {error}"))?;
    fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .await
        .map_err(|error| format!("Could not secure pin configuration directory: {error}"))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "System time is before the Unix epoch; cannot create a temporary pin file.")?
        .as_nanos();
    let temporary = parent.join(format!(
        ".pins-{}-{stamp}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = async {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .await
            .map_err(|error| format!("Could not create temporary pin file: {error}"))?;
        file.write_all(&bytes)
            .await
            .map_err(|error| format!("Could not write pins: {error}"))?;
        file.sync_all()
            .await
            .map_err(|error| format!("Could not persist pins: {error}"))?;
        fs::rename(&temporary, path)
            .await
            .map_err(|error| format!("Could not replace saved pins: {error}"))?;
        fs::File::open(parent)
            .await
            .map_err(|error| format!("Could not open configuration directory: {error}"))?
            .sync_all()
            .await
            .map_err(|error| {
                format!("Pins were written, but directory synchronization failed: {error}")
            })
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Scope;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn corrupted_or_unsupported_config_is_not_silently_reset() {
        for bytes in [
            b"{broken".as_slice(),
            br#"{"version":2,"pins":[]}"#,
            br#"{"version":1,"pins":[{"scope":"system","name":"/tmp/evil.service"}]}"#,
        ] {
            assert!(decode(bytes).is_err());
        }
    }

    #[test]
    fn duplicate_pins_are_rejected_without_conflating_scopes() {
        let loaded = decode(br#"{"version":1,"pins":[{"scope":"user","name":"example.service"},{"scope":"system","name":"example.service"}]}"#).unwrap();
        assert_eq!(
            loaded.iter().map(|id| id.scope).collect::<Vec<_>>(),
            [Scope::User, Scope::System]
        );
        assert!(decode(br#"{"version":1,"pins":[{"scope":"user","name":"example.service"},{"scope":"user","name":"example.service"}]}"#).is_err());
    }

    #[test]
    fn saving_pins_preserves_order_and_invalid_updates_leave_previous_file() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = Scratch(env::temp_dir().join(format!(
                "cosmos-applet-systemd-config-test-{}-{stamp}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            )));
            fs::create_dir(&root.0).await.unwrap();
            let path = root.0.join("pins.json");
            let pins = vec![
                UnitId {
                    scope: Scope::User,
                    name: "z.service".into(),
                },
                UnitId {
                    scope: Scope::System,
                    name: "a.timer".into(),
                },
            ];
            save_to(&path, &pins).await.unwrap();
            assert_eq!(load_from(&path).await.unwrap(), pins);
            let invalid = vec![UnitId {
                scope: Scope::User,
                name: "../bad.service".into(),
            }];
            assert!(save_to(&path, &invalid).await.is_err());
            assert_eq!(load_from(&path).await.unwrap(), pins);
        });
    }
}
