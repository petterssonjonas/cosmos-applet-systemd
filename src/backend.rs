// SPDX-License-Identifier: MPL-2.0

use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    path::PathBuf,
    time::Duration,
};

use futures_util::{StreamExt, future, stream};
use serde::Serialize;
use tokio::time::timeout;
use zbus::{
    Proxy,
    proxy::MethodFlags,
    zvariant::{DynamicDeserialize, DynamicType, OwnedObjectPath, OwnedValue},
};

use crate::model::{Scope, UnitAction, UnitCatalog, UnitId, UnitListing, UnitState};

mod quadlet;

const DESTINATION: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_INTERFACE: &str = "org.freedesktop.systemd1.Manager";
const UNIT_INTERFACE: &str = "org.freedesktop.systemd1.Unit";
const READ_TIMEOUT: Duration = Duration::from_secs(15);
const AUTH_TIMEOUT: Duration = Duration::from_secs(120);
const JOB_TIMEOUT: Duration = Duration::from_secs(120);
const STATUS_CONCURRENCY: usize = 8;

type LoadedUnit = (
    String,
    String,
    String,
    String,
    String,
    String,
    OwnedObjectPath,
    u32,
    String,
    OwnedObjectPath,
);
type FileChange = (String, String, String);

async fn bounded<T>(
    operation: &str,
    pending: impl Future<Output = zbus::Result<T>>,
) -> Result<T, String> {
    timeout(READ_TIMEOUT, pending)
        .await
        .map_err(|_| {
            format!(
                "{operation} timed out after {} seconds.",
                READ_TIMEOUT.as_secs()
            )
        })?
        .map_err(|error| format!("{operation}: {error}"))
}

async fn manager(scope: Scope) -> Result<Proxy<'static>, String> {
    let builder = match scope {
        Scope::System => zbus::connection::Builder::system(),
        Scope::User => zbus::connection::Builder::session(),
    }
    .map_err(|error| format!("{} bus: {error}", scope.label()))?;
    let connection = bounded(
        "Connecting to the D-Bus",
        builder.method_timeout(AUTH_TIMEOUT + READ_TIMEOUT).build(),
    )
    .await?;
    bounded(
        "Creating systemd manager proxy",
        Proxy::new(&connection, DESTINATION, MANAGER_PATH, MANAGER_INTERFACE),
    )
    .await
}

async fn read_call<B, R>(proxy: &Proxy<'_>, method: &str, body: &B) -> Result<R, String>
where
    B: Serialize + DynamicType,
    R: for<'de> DynamicDeserialize<'de>,
{
    bounded(method, proxy.call(method, body)).await
}

async fn mutate<B, R>(proxy: &Proxy<'_>, method: &str, body: &B) -> Result<R, String>
where
    B: Serialize + DynamicType,
    R: for<'de> DynamicDeserialize<'de>,
{
    let reply = timeout(AUTH_TIMEOUT, proxy.call_with_flags(method, MethodFlags::AllowInteractiveAuth.into(), body)).await
        .map_err(|_| format!("{method} timed out after {} seconds. The request may still complete and its job may still continue. Refresh status before retrying.", AUTH_TIMEOUT.as_secs()))?
        .map_err(|error| format!("{method}: {error}"))?;
    reply.ok_or_else(|| format!("{method} returned no reply; the outcome is unknown."))
}

/// Lists ordinary units without loading every installed file, and verifies generated
/// user services against their authoritative SourcePath before linking Quadlet sources.
pub async fn catalog(scope: Scope) -> Result<UnitCatalog, String> {
    let manager = manager(scope).await?;
    let (loaded, installed): (Vec<LoadedUnit>, Vec<(String, String)>) = future::try_join(
        read_call(&manager, "ListUnits", &()),
        read_call(&manager, "ListUnitFiles", &()),
    )
    .await?;
    let mut entries = BTreeMap::new();
    let mut loaded_paths = HashMap::new();
    let mut installed_paths = HashMap::new();
    for (name, description, _, active_state, sub_state, _, path, _, _, _) in loaded {
        let id = UnitId { scope, name };
        if id.validate().is_err() {
            continue;
        }
        if scope == Scope::User {
            loaded_paths.insert(id.name.clone(), path);
        }
        entries.insert(
            id.name.clone(),
            UnitListing {
                id,
                description,
                active_state,
                sub_state,
                unit_file_state: "unknown".into(),
                quadlet_source: None,
            },
        );
    }
    for (path, file_state) in installed {
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let id = UnitId { scope, name };
        if id.validate().is_err() {
            continue;
        }
        if scope == Scope::User {
            installed_paths.insert(id.name.clone(), PathBuf::from(path));
        }
        match entries.entry(id.name.clone()) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().unit_file_state = file_state
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(UnitListing {
                    id,
                    description: "unknown".into(),
                    active_state: "unknown".into(),
                    sub_state: "unknown".into(),
                    unit_file_state: file_state,
                    quadlet_source: None,
                });
            }
        }
    }
    let mut catalog = UnitCatalog {
        units: entries.into_values().collect(),
        quadlets: Vec::new(),
        quadlet_directory: None,
        quadlet_error: None,
    };
    if scope != Scope::User {
        return Ok(catalog);
    }
    let discovery_deadline = tokio::time::Instant::now() + READ_TIMEOUT;
    let discovery = tokio::task::spawn_blocking(quadlet::discover);
    let sources: Vec<_> = stream::iter(0..catalog.units.len())
        .filter_map(|index| {
            let unit = &catalog.units[index];
            let instance_template = service_template(&unit.id.name);
            let generated_instance = loaded_paths.contains_key(&unit.id.name)
                && instance_template.as_ref().is_some_and(|template| {
                    catalog
                        .units
                        .binary_search_by(|unit| unit.id.name.cmp(template))
                        .is_ok_and(|index| catalog.units[index].unit_file_state == "generated")
                });
            future::ready(
                (unit.id.name.ends_with(".service")
                    && (unit.unit_file_state == "generated"
                        || unit.unit_file_state == "alias"
                        || generated_instance))
                    .then_some(index),
            )
        })
        .map(|index| {
            let unit = &catalog.units[index];
            let manager = &manager;
            let loaded_path = loaded_paths.get(&unit.id.name);
            let installed_path = installed_paths.get(&unit.id.name);
            async move {
                let result = timeout(READ_TIMEOUT, async {
                    // An ordinary alias is not a reason to load an unrelated unit.
                    if unit.unit_file_state == "alias" {
                        let Some(path) = installed_path else {
                            return Ok(None);
                        };
                        let canonical = tokio::fs::canonicalize(path).await.map_err(|error| {
                            format!("Cannot resolve alias {}: {error}", unit.id.name)
                        })?;
                        if !quadlet::is_generated_fragment(&canonical) {
                            return Ok(None);
                        }
                    }
                    if unit.id.is_template() {
                        let path = installed_path.ok_or_else(|| {
                            format!(
                                "No generated fragment path is available for {}.",
                                unit.id.name
                            )
                        })?;
                        quadlet::fragment_source(path).await
                    } else {
                        unit_source(manager, &unit.id.name, loaded_path).await
                    }
                })
                .await
                .map_err(|_| {
                    format!(
                        "Reading Quadlet metadata for {} timed out after {} seconds.",
                        unit.id.name,
                        READ_TIMEOUT.as_secs()
                    )
                })
                .and_then(|result| result);
                (index, result)
            }
        })
        .buffered(STATUS_CONCURRENCY)
        .collect()
        .await;
    let mut errors = Vec::new();
    for (index, result) in sources {
        match result {
            Ok(source) => catalog.units[index].quadlet_source = source,
            Err(error) => errors.push(format!("{}: {error}", catalog.units[index].id.name)),
        }
    }
    match tokio::time::timeout_at(discovery_deadline, discovery).await {
        Ok(Ok(mut discovery)) => {
            catalog.quadlet_directory = discovery.directory.take();
            let resolved: Vec<_> = stream::iter(0..catalog.units.len())
                .map(|index| {
                    let unit = &catalog.units[index];
                    async move {
                        let source = unit.quadlet_source.as_ref()?;
                        let result = timeout(READ_TIMEOUT, tokio::fs::canonicalize(source))
                            .await
                            .map_err(|_| {
                                format!(
                                    "Resolving SourcePath for {} timed out after {} seconds.",
                                    unit.id.name,
                                    READ_TIMEOUT.as_secs()
                                )
                            })
                            .and_then(|result| {
                                result.map_err(|error| {
                                    format!(
                                        "Cannot resolve SourcePath for {} ({}): {error}",
                                        unit.id.name,
                                        source.display()
                                    )
                                })
                            });
                        Some((index, result))
                    }
                })
                .buffered(STATUS_CONCURRENCY)
                .filter_map(future::ready)
                .collect()
                .await;
            for (index, result) in resolved {
                match result {
                    Ok(source) => discovery.link_canonical(index, &source),
                    Err(error) => errors.push(error),
                }
            }
            catalog.quadlets = discovery.files;
            errors.extend(discovery.errors);
        }
        Ok(Err(error)) => errors.push(format!("Quadlet discovery failed: {error}")),
        Err(_) => errors.push(format!(
            "Quadlet discovery timed out after {} seconds.",
            READ_TIMEOUT.as_secs()
        )),
    }
    if !errors.is_empty() {
        catalog.quadlet_error = Some(errors.join("\n"));
    }
    Ok(catalog)
}

fn service_template(name: &str) -> Option<String> {
    let (base, instance) = name.strip_suffix(".service")?.split_once('@')?;
    (!instance.is_empty()).then(|| format!("{base}@.service"))
}

async fn unit_source(
    manager: &Proxy<'_>,
    name: &str,
    loaded_path: Option<&OwnedObjectPath>,
) -> Result<Option<PathBuf>, String> {
    let path = match loaded_path {
        Some(path) => path.clone(),
        None => read_call(manager, "LoadUnit", &(name,)).await?,
    };
    let proxy = bounded(
        "Creating unit source proxy",
        Proxy::new(
            manager.connection(),
            DESTINATION,
            path,
            "org.freedesktop.DBus.Properties",
        ),
    )
    .await?;
    let value: OwnedValue = read_call(&proxy, "Get", &(UNIT_INTERFACE, "SourcePath")).await?;
    let source = String::try_from(value)
        .map_err(|error| format!("Invalid systemd SourcePath for {name}: {error}"))?;
    quadlet::source_path(&source)
}

async fn template_source(
    manager: &Proxy<'_>,
    id: &UnitId,
    unit_file_state: &str,
) -> Result<Option<PathBuf>, String> {
    if !matches!(unit_file_state, "generated" | "alias") || !id.name.ends_with(".service") {
        return Ok(None);
    }
    let states: &[&str] = &[];
    let names = [id.name.as_str()];
    let installed: Vec<(String, String)> = read_call(
        manager,
        "ListUnitFilesByPatterns",
        &(states, names.as_slice()),
    )
    .await?;
    let (path, _) = installed
        .into_iter()
        .find(|(path, _)| path.rsplit('/').next() == Some(id.name.as_str()))
        .ok_or_else(|| format!("No fragment path is available for {}.", id.name))?;
    if unit_file_state == "alias" {
        let canonical = tokio::fs::canonicalize(&path)
            .await
            .map_err(|error| format!("Cannot resolve template alias {}: {error}", id.name))?;
        if !quadlet::is_generated_fragment(&canonical) {
            return Ok(None);
        }
    }
    quadlet::fragment_source(std::path::Path::new(&path)).await
}

/// Reads each pin independently, preserving input IDs and order, with one connection per scope.
pub async fn statuses(pins: Vec<UnitId>) -> Vec<(UnitId, Result<UnitState, String>)> {
    let needed = |scope| {
        pins.iter()
            .any(|id| id.scope == scope && id.validate().is_ok())
    };
    let connect = |needed, scope| async move {
        if needed {
            manager(scope).await.map(Some)
        } else {
            Ok(None)
        }
    };
    let (system, user) = future::join(
        connect(needed(Scope::System), Scope::System),
        connect(needed(Scope::User), Scope::User),
    )
    .await;
    stream::iter(pins)
        .map(|id| {
            let connection = match id.scope {
                Scope::System => &system,
                Scope::User => &user,
            };
            async move {
                let result = match id.validate() {
                    Err(error) => Err(error),
                    Ok(()) => match connection {
                        Ok(Some(manager)) => read_state(manager, &id).await.map(|(state, _)| state),
                        Err(error) => Err(error.clone()),
                        Ok(None) => {
                            Err("No systemd connection is available for this scope.".into())
                        }
                    },
                };
                (id, result)
            }
        })
        .buffered(STATUS_CONCURRENCY)
        .collect()
        .await
}

fn property<T>(properties: &mut HashMap<String, OwnedValue>, name: &str) -> Result<T, String>
where
    T: TryFrom<OwnedValue>,
    T::Error: std::fmt::Display,
{
    let value = properties
        .remove(name)
        .ok_or_else(|| format!("systemd did not return required property {name}."))?;
    T::try_from(value).map_err(|error| format!("Invalid systemd property {name}: {error}"))
}

fn file_capabilities(state: &str) -> (bool, bool) {
    let enable = matches!(
        state,
        "disabled" | "enabled-runtime" | "linked" | "linked-runtime" | "indirect"
    );
    let disable = matches!(
        state,
        "enabled" | "enabled-runtime" | "linked" | "linked-runtime" | "indirect"
    );
    (enable, disable)
}

fn runtime_capabilities(
    template: bool,
    load_state: &str,
    file_state: &str,
    can_start: bool,
    can_stop: bool,
    refuse_start: bool,
    refuse_stop: bool,
) -> (bool, bool, bool) {
    let start = !template
        && load_state == "loaded"
        && !matches!(file_state, "masked" | "masked-runtime")
        && can_start
        && !refuse_start;
    // Masking or a failed configuration reload does not prevent stopping an already running unit.
    let stop = !template && can_stop && !refuse_stop;
    (start, start && stop, stop)
}

fn is_missing_file(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::MethodError(name, _, _) => matches!(
            name.as_str(),
            "org.freedesktop.systemd1.NoSuchUnit" | "org.freedesktop.DBus.Error.FileNotFound"
        ),
        zbus::Error::FDO(error) => matches!(error.as_ref(), zbus::fdo::Error::FileNotFound(_)),
        _ => false,
    }
}

async fn file_state(manager: &Proxy<'_>, name: &str) -> Result<String, String> {
    let result = timeout(
        READ_TIMEOUT,
        manager.call::<_, _, String>("GetUnitFileState", &(name,)),
    )
    .await
    .map_err(|_| {
        format!(
            "GetUnitFileState timed out after {} seconds.",
            READ_TIMEOUT.as_secs()
        )
    })?;
    match result {
        Ok(state) if state.is_empty() => Ok("unknown".into()),
        Ok(state) => Ok(state),
        Err(error) if is_missing_file(&error) => Ok("not-found".into()),
        Err(error) => Err(format!("GetUnitFileState({name}): {error}")),
    }
}

async fn read_state(manager: &Proxy<'_>, id: &UnitId) -> Result<(UnitState, String), String> {
    id.validate()?;
    if id.is_template() {
        let unit_file_state = file_state(manager, &id.name).await?;
        let quadlet_source = timeout(READ_TIMEOUT, template_source(manager, id, &unit_file_state))
            .await
            .map_err(|_| {
                format!(
                    "Reading SourcePath for {} timed out after {} seconds.",
                    id.name,
                    READ_TIMEOUT.as_secs()
                )
            })??;
        let (can_enable, can_disable) = if quadlet_source.is_some() {
            (false, false)
        } else {
            file_capabilities(&unit_file_state)
        };
        return Ok((
            UnitState {
                description: "Template unit; select an instance for runtime actions.".into(),
                load_state: "template".into(),
                active_state: "not-applicable".into(),
                sub_state: "not-applicable".into(),
                unit_file_state,
                quadlet_source,
                can_start: false,
                can_restart: false,
                can_stop: false,
                can_enable,
                can_disable,
            },
            id.name.clone(),
        ));
    }
    let path: OwnedObjectPath = read_call(manager, "LoadUnit", &(id.name.as_str(),)).await?;
    let properties_proxy = bounded(
        "Creating unit properties proxy",
        Proxy::new(
            manager.connection(),
            DESTINATION,
            path,
            "org.freedesktop.DBus.Properties",
        ),
    )
    .await?;
    let mut properties: HashMap<String, OwnedValue> =
        read_call(&properties_proxy, "GetAll", &(UNIT_INTERFACE,)).await?;
    let canonical_name: String = property(&mut properties, "Id")?;
    let description = property(&mut properties, "Description")?;
    let load_state: String = property(&mut properties, "LoadState")?;
    let active_state = property(&mut properties, "ActiveState")?;
    let sub_state = property(&mut properties, "SubState")?;
    let reported_file_state: String = property(&mut properties, "UnitFileState")?;
    let fragment_path: String = property(&mut properties, "FragmentPath")?;
    let source_path: String = property(&mut properties, "SourcePath")?;
    let quadlet_source = quadlet::source_path(&source_path)?;
    let transient: bool = property(&mut properties, "Transient")?;
    let manager_can_start = property(&mut properties, "CanStart")?;
    let manager_can_stop = property(&mut properties, "CanStop")?;
    let refuse_start = property(&mut properties, "RefuseManualStart")?;
    let refuse_stop = property(&mut properties, "RefuseManualStop")?;
    // Unit.UnitFileState refers to FragmentPath, which can be a template or an alias target.
    // Query the requested name instead, so instances and aliases keep their own file state.
    let unit_file_state = if transient {
        "transient".into()
    } else if load_state == "not-found" && fragment_path.is_empty() {
        "not-found".into()
    } else if fragment_path.is_empty() && reported_file_state.is_empty() {
        "not-applicable".into()
    } else {
        file_state(manager, &id.name).await?
    };
    let (can_start, can_restart, can_stop) = runtime_capabilities(
        false,
        &load_state,
        &unit_file_state,
        manager_can_start,
        manager_can_stop,
        refuse_start,
        refuse_stop,
    );
    let (can_enable, can_disable) =
        if quadlet_source.is_some() || reported_file_state == "generated" {
            (false, false)
        } else {
            file_capabilities(&unit_file_state)
        };
    Ok((
        UnitState {
            description,
            load_state,
            active_state,
            sub_state,
            unit_file_state,
            quadlet_source,
            can_start,
            can_restart,
            can_stop,
            can_enable,
            can_disable,
        },
        canonical_name,
    ))
}

/// Revalidates the named unit and its current capabilities before requesting a mutation.
/// Runtime success means the corresponding systemd job finished with result `done`.
pub async fn perform(id: UnitId, action: UnitAction) -> Result<String, String> {
    id.validate()?;
    if id.is_template()
        && matches!(
            action,
            UnitAction::Start | UnitAction::Restart | UnitAction::Stop
        )
    {
        return Err(format!(
            "{} is a template. Select a named instance before {}.",
            id.name,
            action.label()
        ));
    }
    let manager = manager(id.scope).await?;
    let (state, canonical_name) = read_state(&manager, &id).await?;
    if !state.allows(action) {
        if matches!(action, UnitAction::Enable | UnitAction::Disable)
            && (state.quadlet_source.is_some() || state.unit_file_state == "generated")
        {
            let reload = match id.scope {
                Scope::User => "systemctl --user daemon-reload",
                Scope::System => "systemctl daemon-reload",
            };
            return Err(format!(
                "{} is generator-managed. Configure autostart in the source file's [Install] section, then run `{reload}`. Generated services cannot be enabled or disabled directly; no files were changed.",
                id.name
            ));
        }
        return Err(format!(
            "{} is unavailable for {} (load state: {}; unit-file state: {}). The unit must support the action and permit manual operation.",
            action.label(),
            id.name,
            state.load_state,
            state.unit_file_state
        ));
    }
    match action {
        UnitAction::Start => runtime_job(&manager, &id, &canonical_name, action, "StartUnit").await,
        UnitAction::Restart => {
            runtime_job(&manager, &id, &canonical_name, action, "RestartUnit").await
        }
        UnitAction::Stop => runtime_job(&manager, &id, &canonical_name, action, "StopUnit").await,
        UnitAction::Enable => change_files(&manager, &id, true).await,
        UnitAction::Disable => change_files(&manager, &id, false).await,
    }
}

async fn runtime_job(
    manager: &Proxy<'_>,
    id: &UnitId,
    canonical_name: &str,
    action: UnitAction,
    method: &str,
) -> Result<String, String> {
    // Register the bus match and enable systemd signals before dispatch, including for fast jobs.
    let mut completed = bounded(
        "Subscribing to JobRemoved",
        manager.receive_signal_with_args("JobRemoved", &[(2, canonical_name)]),
    )
    .await?;
    let _: () = read_call(manager, "Subscribe", &()).await?;
    let job: OwnedObjectPath = mutate(manager, method, &(id.name.as_str(), "replace")).await?;
    let outcome = timeout(JOB_TIMEOUT, async {
        while let Some(message) = completed.next().await {
            let (_, path, _, result): (u32, OwnedObjectPath, String, String) = message.body().deserialize()
                .map_err(|error| format!("Cannot read JobRemoved: {error}. The job may still continue."))?;
            if path == job {
                return if result == "done" {
                    Ok(format!("{} completed for {} (job result: done).", action.label(), id.name))
                } else {
                    Err(format!("{} failed for {}: systemd job result is {result}.", action.label(), id.name))
                };
            }
        }
        Err(format!("JobRemoved subscription ended before {} completed. The job may still continue; refresh status.", id.name))
    }).await;
    outcome.map_err(|_| format!("Timed out after {} seconds waiting for {} on {}. The job may still continue; refresh status before retrying.", JOB_TIMEOUT.as_secs(), action.label(), id.name))?
}

async fn change_files(manager: &Proxy<'_>, id: &UnitId, enable: bool) -> Result<String, String> {
    let files = [id.name.as_str()];
    let action = if enable { "Enable" } else { "Disable" };
    // systemd emits this before its method reply even when an operation only partially succeeds.
    let mut changed = bounded(
        "Subscribing to UnitFilesChanged",
        manager.receive_signal("UnitFilesChanged"),
    )
    .await?;
    let mut changes = Vec::new();
    let result: Result<bool, String> = if enable {
        // A slice has D-Bus signature `as`; a Rust fixed array would be a structure.
        // Enable persistently, without overwriting existing links or unmasking.
        mutate::<_, (bool, Vec<FileChange>)>(
            manager,
            "EnableUnitFiles",
            &(files.as_slice(), false, false),
        )
        .await
        .map(|(has_install_info, updated)| {
            changes = updated;
            has_install_info
        })
    } else {
        async {
            // A unit may have both persistent and runtime activation links.
            for runtime in [false, true] {
                let updated: Vec<FileChange> =
                    mutate(manager, "DisableUnitFiles", &(files.as_slice(), runtime)).await?;
                changes.extend(updated);
            }
            Ok(true)
        }
        .await
    };
    let has_install_info = match result {
        Ok(has_install_info) => has_install_info,
        Err(error) => {
            if changes
                .iter()
                .any(|(kind, _, _)| matches!(kind.as_str(), "symlink" | "unlink"))
                || futures_util::FutureExt::now_or_never(changed.next())
                    .flatten()
                    .is_some()
            {
                let reload: Result<(), String> = mutate(manager, "Reload", &()).await;
                reload.map_err(|reload_error| format!("{error}. Unit-file links may have changed, and manager reload failed: {reload_error}. Refresh status before retrying."))?;
                return Err(format!(
                    "{error}. Unit-file links may have changed; manager configuration was reloaded. Refresh status before retrying."
                ));
            }
            return Err(error);
        }
    };
    let link_changes = changes
        .iter()
        .filter(|(kind, _, _)| matches!(kind.as_str(), "symlink" | "unlink"))
        .count();
    if link_changes > 0 {
        let reload: Result<(), String> = mutate(manager, "Reload", &()).await;
        reload.map_err(|error| format!("{action} changed {link_changes} unit-file links for {}, but manager reload failed: {error}. Changes remain on disk; runtime activity was not changed by this action.", id.name))?;
    }
    let state = file_state(manager, &id.name).await
        .map_err(|error| format!("{action} completed for {}, but the resulting unit-file state could not be read: {error}", id.name))?;
    if !has_install_info {
        return Err(format!(
            "{} has no [Install] enablement instructions (unit-file state: {state}; {link_changes} link changes). Runtime activity was not changed.",
            id.name
        ));
    }
    if !enable && matches!(state.as_str(), "enabled" | "enabled-runtime") {
        return Err(format!(
            "Disable removed {link_changes} local links for {}, but systemd still reports {state}. Global configuration or another process may keep it enabled. No unit was masked; runtime activity was not changed.",
            id.name
        ));
    }
    Ok(format!(
        "{action} completed for {}. Unit-file state: {state}; {link_changes} link changes. Runtime activity was not changed.",
        id.name
    ))
}

#[cfg(test)]
mod tests {
    use super::{file_capabilities, runtime_capabilities};

    #[test]
    fn file_states_without_direct_installation_do_not_offer_enable_or_disable() {
        let states = [
            "static",
            "masked",
            "masked-runtime",
            "alias",
            "generated",
            "transient",
            "bad",
            "invalid",
            "not-found",
            "not-applicable",
            "unknown",
            "future-state",
        ];
        assert!(
            states
                .iter()
                .all(|state| file_capabilities(state) == (false, false))
        );
    }

    #[test]
    fn runtime_only_enablement_can_be_made_persistent() {
        assert_eq!(file_capabilities("enabled-runtime"), (true, true));
    }

    #[test]
    fn linked_and_indirect_are_not_treated_as_disabled() {
        assert!(
            ["linked", "linked-runtime", "indirect"]
                .iter()
                .all(|state| file_capabilities(state) == (true, true))
        );
    }

    #[test]
    fn manual_stop_refusal_blocks_restart_but_not_start() {
        assert_eq!(
            runtime_capabilities(false, "loaded", "disabled", true, true, false, true),
            (true, false, false)
        );
    }

    #[test]
    fn masked_running_units_can_still_be_stopped() {
        assert_eq!(
            runtime_capabilities(false, "masked", "masked", true, true, false, false),
            (false, false, true)
        );
    }

    #[test]
    fn static_units_can_have_runtime_actions() {
        assert_eq!(
            runtime_capabilities(false, "loaded", "static", true, true, false, false),
            (true, true, true)
        );
    }

    #[test]
    fn templates_never_offer_runtime_actions() {
        assert_eq!(
            runtime_capabilities(true, "loaded", "disabled", true, true, false, false),
            (false, false, false)
        );
    }
}
