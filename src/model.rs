// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    System,
    User,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::User => "User",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitId {
    pub scope: Scope,
    pub name: String,
}

impl UnitId {
    pub fn validate(&self) -> Result<(), String> {
        let valid_suffix = self.name.rsplit_once('.').is_some_and(|(base, suffix)| {
            !base.is_empty()
                && matches!(
                    suffix,
                    "service"
                        | "socket"
                        | "target"
                        | "device"
                        | "mount"
                        | "automount"
                        | "swap"
                        | "timer"
                        | "path"
                        | "slice"
                        | "scope"
                )
        });
        if self.name.len() > 255
            || !valid_suffix
            || !self
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":_.@\\-".contains(&byte))
        {
            return Err(
                "Invalid unit name: use a full systemd unit name, not a path or pattern.".into(),
            );
        }
        Ok(())
    }

    pub fn is_template(&self) -> bool {
        self.name
            .rsplit_once('.')
            .is_some_and(|(base, _)| base.ends_with('@'))
    }
}

#[derive(Debug, Clone)]
pub struct UnitListing {
    pub id: UnitId,
    pub description: String,
    pub active_state: String,
    pub sub_state: String,
    pub unit_file_state: String,
    pub quadlet_source: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone)]
pub struct UnitState {
    pub description: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub unit_file_state: String,
    pub quadlet_source: Option<std::path::PathBuf>,
    pub can_start: bool,
    pub can_restart: bool,
    pub can_stop: bool,
    pub can_enable: bool,
    pub can_disable: bool,
}

#[derive(Debug, Clone)]
pub struct QuadletFile {
    pub path: std::path::PathBuf,
    /// Indices into the accompanying catalog's units, verified using SourcePath.
    pub unit_indices: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct UnitCatalog {
    pub units: Vec<UnitListing>,
    pub quadlets: Vec<QuadletFile>,
    pub quadlet_directory: Option<std::path::PathBuf>,
    pub quadlet_error: Option<String>,
}

impl UnitState {
    pub fn allows(&self, action: UnitAction) -> bool {
        match action {
            UnitAction::Start => self.can_start,
            UnitAction::Restart => self.can_restart,
            UnitAction::Stop => self.can_stop,
            UnitAction::Enable => self.can_enable,
            UnitAction::Disable => self.can_disable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitAction {
    Start,
    Restart,
    Stop,
    Enable,
    Disable,
}

impl UnitAction {
    pub const ALL: [Self; 5] = [
        Self::Start,
        Self::Restart,
        Self::Stop,
        Self::Enable,
        Self::Disable,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Restart => "Restart",
            Self::Stop => "Stop",
            Self::Enable => "Enable",
            Self::Disable => "Disable",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_unit_names_allow_root_mounts_and_escaped_instances() {
        for name in [
            "-.mount",
            r"dev-disk-by\x2duuid-123.device",
            "example@worker.service",
            "example@.timer",
        ] {
            let id = UnitId {
                scope: Scope::User,
                name: name.into(),
            };
            assert!(id.validate().is_ok(), "{name}");
        }
        let instance = UnitId {
            scope: Scope::User,
            name: "example@worker.service".into(),
        };
        let template = UnitId {
            scope: Scope::User,
            name: "example@.service".into(),
        };
        assert!(!instance.is_template());
        assert!(template.is_template());
    }

    #[test]
    fn paths_patterns_and_control_characters_cannot_target_units() {
        for name in [
            "/tmp/other.service",
            "../other.service",
            "*.service",
            "a\n.service",
            "a\0.service",
            "a.service b.service",
            ".service",
            "missing-suffix",
        ] {
            assert!(
                UnitId {
                    scope: Scope::System,
                    name: name.into()
                }
                .validate()
                .is_err(),
                "{name:?}"
            );
        }
        let oversized = format!("{}.service", "x".repeat(256));
        assert!(
            UnitId {
                scope: Scope::System,
                name: oversized
            }
            .validate()
            .is_err()
        );
    }
}
