// SPDX-License-Identifier: MPL-2.0

use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use crate::model::QuadletFile;

pub(super) struct Discovery {
    pub directory: Option<PathBuf>,
    pub files: Vec<QuadletFile>,
    pub errors: Vec<String>,
    canonical_sources: HashMap<PathBuf, Vec<usize>>,
}

pub(super) fn config_directory(
    config: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<PathBuf, String> {
    if let Some(config) = config.filter(|value| Path::new(value).is_absolute()) {
        return Ok(Path::new(config).join("containers/systemd"));
    }
    home.filter(|value| Path::new(value).is_absolute())
        .map(|home| Path::new(home).join(".config/containers/systemd"))
        .ok_or_else(|| "Cannot locate user Quadlets: neither an absolute XDG_CONFIG_HOME nor HOME is available.".into())
}

pub(super) fn discover() -> Discovery {
    let config = std::env::var_os("XDG_CONFIG_HOME");
    let home = std::env::var_os("HOME");
    match config_directory(config.as_deref(), home.as_deref()) {
        Ok(directory) => scan(directory),
        Err(error) => Discovery {
            directory: None,
            files: Vec::new(),
            errors: vec![error],
            canonical_sources: HashMap::new(),
        },
    }
}

pub(super) fn source_path(value: &str) -> Result<Option<PathBuf>, String> {
    if !is_source(Path::new(value)) {
        return Ok(None);
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!(
            "systemd returned a non-absolute Quadlet SourcePath: {value}"
        ));
    }
    Ok(Some(path))
}

fn is_source(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            matches!(
                extension,
                "container"
                    | "pod"
                    | "kube"
                    | "network"
                    | "volume"
                    | "image"
                    | "build"
                    | "artifact"
            )
        })
}

pub(super) fn is_generated_fragment(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("service"))
        && path.parent().and_then(Path::file_name).is_some_and(|name| {
            matches!(
                name.to_str(),
                Some("generator" | "generator.early" | "generator.late")
            )
        })
}

fn scan(directory: PathBuf) -> Discovery {
    let mut paths = Vec::new();
    let mut errors = Vec::new();
    let deadline = Instant::now() + super::READ_TIMEOUT;
    match fs::read_dir(&directory) {
        Ok(entries) => {
            if walk(
                &directory,
                entries,
                &mut HashSet::new(),
                &mut paths,
                &mut errors,
                deadline,
            ) {
                errors.push(format!(
                    "Quadlet discovery exceeded {} seconds; the source list is incomplete.",
                    super::READ_TIMEOUT.as_secs()
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => errors.push(format!("Cannot scan {}: {error}", directory.display())),
    }
    paths.sort_by(|(path, _), (other, _)| path.cmp(other));
    let mut canonical_sources: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    let files = paths
        .into_iter()
        .enumerate()
        .map(|(index, (path, canonical))| {
            canonical_sources.entry(canonical).or_default().push(index);
            QuadletFile {
                path,
                unit_indices: Vec::new(),
            }
        })
        .collect();
    Discovery {
        directory: Some(directory),
        files,
        errors,
        canonical_sources,
    }
}

fn walk(
    directory: &Path,
    entries: fs::ReadDir,
    ancestors: &mut HashSet<PathBuf>,
    paths: &mut Vec<(PathBuf, PathBuf)>,
    errors: &mut Vec<String>,
    deadline: Instant,
) -> bool {
    if Instant::now() >= deadline {
        return true;
    }
    let canonical = match fs::canonicalize(directory) {
        Ok(path) => path,
        Err(error) => {
            errors.push(format!("Cannot resolve {}: {error}", directory.display()));
            return false;
        }
    };
    // Track ancestors, not all visited directories: distinct symlink paths remain visible.
    if !ancestors.insert(canonical.clone()) {
        return false;
    }
    for entry in entries {
        if Instant::now() >= deadline {
            ancestors.remove(&canonical);
            return true;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!("Cannot scan {}: {error}", directory.display()));
                continue;
            }
        };
        let path = entry.path();
        let metadata = entry.file_type().and_then(|kind| {
            if kind.is_symlink() {
                fs::metadata(&path).map(|metadata| metadata.file_type())
            } else {
                Ok(kind)
            }
        });
        match metadata {
            Ok(kind) if kind.is_dir() => match fs::read_dir(&path) {
                Ok(entries) => {
                    if walk(&path, entries, ancestors, paths, errors, deadline) {
                        ancestors.remove(&canonical);
                        return true;
                    }
                }
                Err(error) => errors.push(format!("Cannot scan {}: {error}", path.display())),
            },
            Ok(kind) if kind.is_file() && is_source(&path) => match fs::canonicalize(&path) {
                Ok(canonical) => paths.push((path, canonical)),
                Err(error) => errors.push(format!("Cannot resolve {}: {error}", path.display())),
            },
            Ok(_) => {}
            Err(error) => errors.push(format!("Cannot inspect {}: {error}", path.display())),
        }
    }
    ancestors.remove(&canonical);
    false
}

impl Discovery {
    pub fn link_canonical(&mut self, unit_index: usize, source: &Path) {
        if let Some(indices) = self.canonical_sources.get(source) {
            for &index in indices {
                self.files[index].unit_indices.push(unit_index);
            }
        }
    }

    #[cfg(test)]
    fn link(&mut self, units: &[crate::model::UnitListing]) {
        for (index, unit) in units.iter().enumerate() {
            if let Some(source) = &unit.quadlet_source {
                self.link_canonical(index, &fs::canonicalize(source).unwrap());
            }
        }
    }
}

/// Only generated service fragments are read; Quadlet source contents are never opened.
pub(super) async fn fragment_source(path: &Path) -> Result<Option<PathBuf>, String> {
    let canonical = tokio::fs::canonicalize(path).await.map_err(|error| {
        format!(
            "Cannot resolve generated fragment {}: {error}",
            path.display()
        )
    })?;
    if !is_generated_fragment(&canonical) {
        return Err(format!(
            "Cannot verify Quadlet SourcePath: {} is not a generated service fragment.",
            path.display()
        ));
    }
    let content = tokio::fs::read_to_string(&canonical)
        .await
        .map_err(|error| {
            format!(
                "Cannot read generated fragment {}: {error}",
                canonical.display()
            )
        })?;
    parse_fragment_source(&content).map_err(|error| format!("{}: {error}", canonical.display()))
}

fn parse_fragment_source(content: &str) -> Result<Option<PathBuf>, String> {
    let mut in_unit = false;
    let mut source = None;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_unit = line == "[Unit]";
            continue;
        }
        if !in_unit {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "SourcePath" {
            continue;
        }
        let value = value.trim();
        // Do not guess how escapes/specifiers expand when a template cannot be loaded.
        if value.contains(['\\', '%', '"', '\'']) {
            return Err(
                "Cannot verify escaped or specifier-based SourcePath in a generated template."
                    .into(),
            );
        }
        source = source_path(value)?;
    }
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Scope, UnitId, UnitListing};
    use std::{
        os::unix::fs::symlink,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "cosmos-applet-systemd-quadlet-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn listing(name: &str, source: Option<PathBuf>) -> UnitListing {
        UnitListing {
            id: UnitId {
                scope: Scope::User,
                name: name.into(),
            },
            description: "unknown".into(),
            active_state: "unknown".into(),
            sub_state: "unknown".into(),
            unit_file_state: "generated".into(),
            quadlet_source: source,
        }
    }

    #[test]
    fn discovery_recurses_through_symlinks_without_following_cycles() {
        let root = TestDirectory::new();
        let nested = root.0.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("app.container"), b"not read").unwrap();
        symlink(&root.0, nested.join("cycle")).unwrap();
        symlink(&nested, root.0.join("linked")).unwrap();
        let discovery = scan(root.0.clone());
        assert!(discovery.errors.is_empty(), "{:?}", discovery.errors);
        assert_eq!(
            discovery
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>(),
            vec![
                root.0.join("linked/app.container"),
                nested.join("app.container")
            ]
        );
    }

    #[test]
    fn discovery_ignores_dropins_and_discovers_all_quadlet_types() {
        let root = TestDirectory::new();
        let extensions = [
            "container",
            "pod",
            "kube",
            "network",
            "volume",
            "image",
            "build",
            "artifact",
        ];
        for extension in extensions {
            fs::write(root.0.join(format!("app.{extension}")), b"").unwrap();
        }
        fs::create_dir(root.0.join("app.container.d")).unwrap();
        fs::write(root.0.join("app.container.d/override.conf"), b"").unwrap();
        fs::write(root.0.join("app.service"), b"").unwrap();
        let discovery = scan(root.0.clone());
        assert_eq!(discovery.files.len(), extensions.len());
    }

    #[test]
    fn matching_uses_sourcepath_for_custom_names_aliases_and_symlink_sources() {
        let root = TestDirectory::new();
        let source = root.0.join("app.container");
        fs::write(
            &source,
            b"[Container]\nContainerName=not-the-service\nServiceName=custom",
        )
        .unwrap();
        let linked = root.0.join("linked.container");
        symlink(&source, &linked).unwrap();
        let mut discovery = scan(root.0.clone());
        let units = vec![
            listing("custom.service", Some(source.clone())),
            listing("alias.service", Some(linked)),
            listing("app.service", None),
        ];
        discovery.link(&units);
        assert!(
            discovery
                .files
                .iter()
                .all(|file| file.unit_indices == [0, 1])
        );
    }

    #[test]
    fn unmatched_source_never_gets_a_guessed_service() {
        let root = TestDirectory::new();
        fs::write(root.0.join("app.container"), b"").unwrap();
        let mut discovery = scan(root.0.clone());
        discovery.link(&[listing("app.service", None)]);
        assert!(discovery.files[0].unit_indices.is_empty());
    }

    #[test]
    fn absent_directory_is_empty_but_broken_symlink_reports_an_error() {
        let root = TestDirectory::new();
        assert!(scan(root.0.join("absent")).errors.is_empty());
        symlink(root.0.join("absent"), root.0.join("broken.container")).unwrap();
        let discovery = scan(root.0.clone());
        assert_eq!(discovery.errors.len(), 1);
    }

    #[test]
    fn config_root_respects_absolute_xdg_and_falls_back_for_empty_or_relative_values() {
        let home = Some(OsStr::new("/home/example"));
        assert_eq!(
            config_directory(Some(OsStr::new("/custom")), home).unwrap(),
            PathBuf::from("/custom/containers/systemd")
        );
        for config in [None, Some(OsStr::new("")), Some(OsStr::new("relative"))] {
            assert_eq!(
                config_directory(config, home).unwrap(),
                PathBuf::from("/home/example/.config/containers/systemd")
            );
        }
        assert!(config_directory(None, None).is_err());
    }

    #[test]
    fn template_fragment_mapping_reads_only_unit_sourcepath() {
        assert_eq!(parse_fragment_source("[Unit]\nSourcePath=/config/app@.container\n[Container]\nSourcePath=/wrong.container\n").unwrap(), Some(PathBuf::from("/config/app@.container")));
        assert!(parse_fragment_source("[Unit]\nSourcePath=/config/%i.container").is_err());
        assert!(parse_fragment_source("[Unit]\nSourcePath=relative.container").is_err());
    }

    #[test]
    fn generated_template_alias_reads_verified_fragment_without_reading_source_contents() {
        let root = TestDirectory::new();
        let generated = root.0.join("generator");
        fs::create_dir(&generated).unwrap();
        let source = root.0.join("app@.container");
        fs::write(&source, [0xff, 0xfe]).unwrap();
        let fragment = generated.join("custom@.service");
        fs::write(
            &fragment,
            format!("[Unit]\nSourcePath={}\n", source.display()),
        )
        .unwrap();
        let alias = root.0.join("alias@.service");
        symlink(&fragment, &alias).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(
            runtime.block_on(fragment_source(&alias)).unwrap(),
            Some(source)
        );
    }

    #[test]
    fn template_fallback_rejects_source_files_and_aliases_to_ordinary_services() {
        let root = TestDirectory::new();
        let source = root.0.join("app.container");
        fs::write(&source, b"[Unit]\nSourcePath=/wrong.container").unwrap();
        let ordinary = root.0.join("ordinary@.service");
        fs::write(&ordinary, b"[Unit]\nSourcePath=/wrong.container").unwrap();
        let generated = root.0.join("generator");
        fs::create_dir(&generated).unwrap();
        let alias = generated.join("alias@.service");
        symlink(&ordinary, &alias).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        assert!(runtime.block_on(fragment_source(&source)).is_err());
        assert!(runtime.block_on(fragment_source(&alias)).is_err());
    }

    #[test]
    fn non_directory_config_root_reports_scan_error_without_sources() {
        let root = TestDirectory::new();
        let file = root.0.join("not-a-directory");
        fs::write(&file, b"").unwrap();
        let discovery = scan(file);
        assert!(discovery.files.is_empty());
        assert_eq!(discovery.errors.len(), 1);
    }
}
