// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::{Alignment, Length, Limits, Subscription, window::Id};
use cosmic::prelude::*;
use cosmic::widget;

use crate::model::{QuadletFile, Scope, UnitAction, UnitCatalog, UnitId, UnitListing, UnitState};
use crate::{backend, config};

const PAGE_SIZE: usize = 20;
const POPUP_WIDTH: f32 = 540.0;

pub struct AppModel {
    core: cosmic::Core,
    popup: Option<Id>,
    icon: widget::icon::Handle,
    cancel_id: widget::Id,
    pins: Vec<UnitId>,
    pending_pins: Option<Vec<UnitId>>,
    config_loaded: bool,
    config_loading: bool,
    config_generation: u64,
    config_error: Option<String>,
    settings: bool,
    scope: Scope,
    quadlets_only: bool,
    search: String,
    catalog: Vec<UnitListing>,
    quadlets: Vec<QuadletFile>,
    quadlet_directory: Option<PathBuf>,
    quadlet_error: Option<String>,
    filtered: Vec<usize>,
    catalog_page: usize,
    pin_page: usize,
    catalog_loading: bool,
    catalog_generation: u64,
    catalog_error: Option<String>,
    states: HashMap<UnitId, Result<UnitState, String>>,
    status_loading: bool,
    status_generation: u64,
    confirmation: Option<(UnitId, UnitAction)>,
    busy: Option<(UnitId, UnitAction)>,
    action_generation: u64,
    notice: Option<(String, bool)>,
}

#[derive(Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    Tick,
    Refresh,
    OpenSettings,
    Back,
    OpenQuadlets,
    ScopeChanged(Scope),
    SearchChanged(String),
    CatalogPage(usize),
    PinPage(usize),
    RetryConfig,
    ConfigLoaded(u64, Result<Vec<UnitId>, String>),
    TogglePin(UnitId),
    PinsSaved(u64, Result<(), String>),
    CatalogFetched(Scope, u64, Result<UnitCatalog, String>),
    StatusesFetched(u64, Vec<(UnitId, Result<UnitState, String>)>),
    RequestAction(UnitId, UnitAction),
    CancelAction,
    ConfirmAction,
    ActionFinished(u64, UnitId, UnitAction, Result<String, String>),
    SurfaceAction(cosmic::surface::Action<Message>),
}

impl std::fmt::Debug for Message {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TogglePopup => "TogglePopup",
            Self::PopupClosed(_) => "PopupClosed",
            Self::Tick => "Tick",
            Self::Refresh => "Refresh",
            Self::OpenSettings => "OpenSettings",
            Self::Back => "Back",
            Self::OpenQuadlets => "OpenQuadlets",
            Self::ScopeChanged(_) => "ScopeChanged",
            Self::SearchChanged(_) => "SearchChanged",
            Self::CatalogPage(_) => "CatalogPage",
            Self::PinPage(_) => "PinPage",
            Self::RetryConfig => "RetryConfig",
            Self::ConfigLoaded(..) => "ConfigLoaded",
            Self::TogglePin(_) => "TogglePin",
            Self::PinsSaved(..) => "PinsSaved",
            Self::CatalogFetched(..) => "CatalogFetched",
            Self::StatusesFetched(..) => "StatusesFetched",
            Self::RequestAction(..) => "RequestAction",
            Self::CancelAction => "CancelAction",
            Self::ConfirmAction => "ConfirmAction",
            Self::ActionFinished(..) => "ActionFinished",
            Self::SurfaceAction(_) => "SurfaceAction",
        })
    }
}

impl AppModel {
    fn new(core: cosmic::Core) -> Self {
        Self {
            core,
            popup: None,
            icon: widget::icon::from_svg_bytes(include_bytes!("../resources/icon.svg"))
                .symbolic(true),
            cancel_id: widget::Id::unique(),
            pins: Vec::new(),
            pending_pins: None,
            config_loaded: false,
            config_loading: false,
            config_generation: 0,
            config_error: None,
            settings: false,
            scope: Scope::System,
            quadlets_only: false,
            search: String::new(),
            catalog: Vec::new(),
            quadlets: Vec::new(),
            quadlet_directory: None,
            quadlet_error: None,
            filtered: Vec::new(),
            catalog_page: 0,
            pin_page: 0,
            catalog_loading: false,
            catalog_generation: 0,
            catalog_error: None,
            states: HashMap::new(),
            status_loading: false,
            status_generation: 0,
            confirmation: None,
            busy: None,
            action_generation: 0,
            notice: None,
        }
    }

    fn load_config(&mut self) -> Task<cosmic::Action<Message>> {
        if self.config_loading || self.pending_pins.is_some() {
            return Task::none();
        }
        self.config_loading = true;
        self.config_generation += 1;
        let generation = self.config_generation;
        Task::perform(config::load(), move |result| {
            cosmic::Action::App(Message::ConfigLoaded(generation, result))
        })
    }

    fn invalidate_statuses(&mut self) {
        self.status_generation += 1;
        self.status_loading = false;
        self.states.clear();
    }

    fn fetch_statuses(&mut self) -> Task<cosmic::Action<Message>> {
        if self.busy.is_some() || self.status_loading || self.pins.is_empty() {
            return Task::none();
        }
        self.status_loading = true;
        self.status_generation += 1;
        let generation = self.status_generation;
        let pins = self.pins.clone();
        Task::perform(backend::statuses(pins), move |result| {
            cosmic::Action::App(Message::StatusesFetched(generation, result))
        })
    }

    fn apply_statuses(
        &mut self,
        generation: u64,
        results: Vec<(UnitId, Result<UnitState, String>)>,
    ) {
        if generation != self.status_generation || !self.status_loading || self.busy.is_some() {
            return;
        }
        self.status_loading = false;
        self.states.clear();
        self.states.extend(results);
        for id in &self.pins {
            self.states.entry(id.clone()).or_insert_with(|| {
                Err(
                    "The status request returned no information for this unit. Refresh to retry."
                        .into(),
                )
            });
        }
    }

    fn invalidate_catalog(&mut self) {
        self.catalog_generation += 1;
        self.catalog_loading = false;
        self.catalog.clear();
        self.quadlets.clear();
        self.quadlet_directory = None;
        self.quadlet_error = None;
        self.filtered.clear();
        self.catalog_page = 0;
    }

    fn fetch_catalog(&mut self) -> Task<cosmic::Action<Message>> {
        self.invalidate_catalog();
        self.catalog_loading = true;
        self.catalog_error = None;
        let scope = self.scope;
        let generation = self.catalog_generation;
        Task::perform(backend::catalog(scope), move |result| {
            cosmic::Action::App(Message::CatalogFetched(scope, generation, result))
        })
    }

    fn filter_catalog(&mut self) {
        let needle = self.search.trim().to_lowercase();
        self.filtered.clear();
        if self.quadlets_only {
            for (index, source) in self.quadlets.iter().enumerate() {
                if needle.is_empty()
                    || source
                        .path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&needle)
                    || source
                        .unit_indices
                        .iter()
                        .filter_map(|index| self.catalog.get(*index))
                        .any(|unit| unit_matches(unit, &needle))
                {
                    self.filtered.push(index);
                }
            }
            self.filtered.sort_unstable_by(|left, right| {
                self.quadlets[*left].path.cmp(&self.quadlets[*right].path)
            });
        } else {
            for (index, unit) in self.catalog.iter().enumerate() {
                if unit_matches(unit, &needle) {
                    self.filtered.push(index);
                }
            }
            // Sort display indices, never the units addressed by Quadlet mappings.
            self.filtered.sort_unstable_by(|left, right| {
                self.catalog[*left]
                    .id
                    .name
                    .cmp(&self.catalog[*right].id.name)
            });
        }
        self.catalog_page = self.catalog_page.min(last_page(self.filtered.len()));
    }

    fn stage_pin_change(&mut self, id: UnitId) -> bool {
        if !self.config_loaded
            || self.config_loading
            || self.pending_pins.is_some()
            || self.busy.is_some()
            || self.confirmation.is_some()
        {
            return false;
        }
        if let Err(error) = id.validate() {
            self.config_error = Some(error);
            return false;
        }
        let mut proposed = self.pins.clone();
        if let Some(index) = proposed.iter().position(|pin| pin == &id) {
            proposed.remove(index);
        } else {
            proposed.push(id);
        }
        self.config_generation += 1;
        self.config_error = None;
        self.pending_pins = Some(proposed);
        true
    }

    fn finish_pin_save(&mut self, generation: u64, result: Result<(), String>) -> bool {
        if generation != self.config_generation {
            return false;
        }
        let Some(proposed) = self.pending_pins.take() else {
            return false;
        };
        match result {
            Ok(()) => {
                self.pins = proposed;
                self.config_error = None;
                self.pin_page = self.pin_page.min(last_page(self.pins.len()));
                self.invalidate_statuses();
                true
            }
            Err(error) => {
                self.config_error = Some(format!(
                    "Saving pins could not be confirmed. Keeping the previous pins in this applet. {error}"
                ));
                false
            }
        }
    }

    fn action_allowed(&self, id: &UnitId, action: UnitAction) -> bool {
        self.busy.is_none()
            && self.pending_pins.is_none()
            && self.pins.contains(id)
            && matches!(self.states.get(id), Some(Ok(state)) if state.allows(action))
    }

    fn pin_view<'a>(&'a self, id: &'a UnitId) -> Element<'a, Message> {
        let mut content = widget::column([])
            .width(Length::Fill)
            .spacing(6)
            .push(
                widget::row([
                    widget::text::title4(&id.name).width(Length::Fill).into(),
                    action_button(
                        "Unpin",
                        Message::TogglePin(id.clone()),
                        self.config_loaded
                            && !self.config_loading
                            && self.pending_pins.is_none()
                            && self.busy.is_none(),
                    ),
                ])
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .push(widget::text(format!("{} scope", id.scope.label())).size(12));
        match self.states.get(id) {
            Some(Ok(state)) => {
                if !state.description.is_empty() {
                    content = content.push(widget::text(&state.description).size(13));
                }
                if let Some(source) = &state.quadlet_source {
                    content = content
                        .push(
                            widget::text(format!("Quadlet source: {}", source.display())).size(12),
                        )
                        .push(widget::text(quadlet_enablement_guidance(id.scope)).size(12));
                }
                content = content.push(indicators(
                    &state.active_state,
                    &state.sub_state,
                    &state.unit_file_state,
                ));
                if state.load_state != "loaded" {
                    content = content.push(colored_text(
                        format!("Load state: {}", state.load_state),
                        Tone::Warning,
                    ));
                }
            }
            Some(Err(error)) => {
                content = content
                    .push(indicators("unknown", "", "unknown"))
                    .push(colored_text(
                        format!("Status unavailable: {error}"),
                        Tone::Error,
                    ));
            }
            None => {
                content = content.push(indicators("unknown", "", "unknown")).push(
                    widget::text(if self.busy.is_some() {
                        "Status is being rechecked after the operation."
                    } else {
                        "Waiting for a current status read."
                    })
                    .size(12),
                );
            }
        }
        let mut buttons = widget::row([]).spacing(6);
        let mut unsupported = Vec::new();
        for action in UnitAction::ALL {
            let enabled = self.action_allowed(id, action);
            let mut button = widget::button::standard(action.label());
            if enabled {
                button = button.on_press(Message::RequestAction(id.clone(), action));
            } else {
                let reason = self.disabled_reason(id, action);
                if let Some(Ok(state)) = self.states.get(id) {
                    let explained_by_quadlet = state.quadlet_source.is_some()
                        && matches!(action, UnitAction::Enable | UnitAction::Disable);
                    if !state.allows(action) && !explained_by_quadlet {
                        unsupported.push(format!("{}: {reason}", action.label()));
                    }
                }
                button = button.tooltip(reason);
            }
            buttons = buttons.push(button);
        }
        content = content.push(buttons);
        if !unsupported.is_empty() {
            content = content.push(widget::text(unsupported.join(" · ")).size(12));
        }
        widget::container(content)
            .padding(10)
            .width(Length::Fill)
            .into()
    }

    fn disabled_reason(&self, id: &UnitId, action: UnitAction) -> String {
        if self.busy.is_some() {
            return "Wait for the current operation and its status refresh.".into();
        }
        if self.pending_pins.is_some() {
            return "Wait for the pin change to finish saving.".into();
        }
        match self.states.get(id) {
            Some(Ok(state)) => match action {
                UnitAction::Enable | UnitAction::Disable if state.quadlet_source.is_some() => {
                    quadlet_enablement_guidance(id.scope)
                }
                UnitAction::Enable | UnitAction::Disable => format!(
                    "Unavailable for this unit's install rules or unit-file state ({}).",
                    file_label(&state.unit_file_state),
                ),
                _ => format!(
                    "Unavailable for this unit's capabilities or load state ({}).",
                    state.load_state,
                ),
            },
            _ => "A successful current status read is required. Use Refresh to retry.".into(),
        }
    }

    fn pins_view(&self) -> Element<'_, Message> {
        let mut content = widget::column([]).width(Length::Fill).spacing(12);
        if self.pins.is_empty() {
            content = content.push(widget::text::title4("No pinned units"))
                .push(widget::text("Open Settings below to find system units, user units, or Quadlet sources and pin the units you want. Nothing is pinned automatically."));
        } else {
            content = content.push(pagination(self.pin_page, self.pins.len(), false));
            for id in self
                .pins
                .iter()
                .skip(self.pin_page * PAGE_SIZE)
                .take(PAGE_SIZE)
            {
                content = content
                    .push(self.pin_view(id))
                    .push(widget::divider::horizontal::default());
            }
            content = content.push(widget::text("Activity and enablement are independent. Status refreshes when opened and every 5 minutes while this popup is open and no operation is running.").size(12));
        }
        content.into()
    }

    fn catalog_unit_view<'a>(
        &'a self,
        unit: &'a UnitListing,
        pin_enabled: bool,
        quadlet_mapping: bool,
    ) -> Element<'a, Message> {
        let mut details = widget::column([]).width(Length::Fill).spacing(4);
        if quadlet_mapping {
            details = details.push(widget::text("Verified generated service:").size(12));
        }
        details = details.push(widget::text(&unit.id.name));
        if !unit.description.is_empty() {
            details = details.push(widget::text(&unit.description).size(12));
        }
        if !quadlet_mapping && let Some(source) = &unit.quadlet_source {
            details = details
                .push(widget::text(format!("Quadlet source: {}", source.display())).size(12));
        }
        details = details.push(
            widget::text(format!(
                "{} scope · Activity: {} · Enablement: {}",
                unit.id.scope.label(),
                activity_label(&unit.active_state, &unit.sub_state),
                file_label(&unit.unit_file_state),
            ))
            .size(12),
        );
        widget::row([
            details.into(),
            action_button(
                if self.pins.contains(&unit.id) {
                    "Unpin"
                } else {
                    "Pin"
                },
                Message::TogglePin(unit.id.clone()),
                pin_enabled,
            ),
        ])
        .align_y(Alignment::Center)
        .spacing(8)
        .into()
    }

    fn settings_view(&self) -> Element<'_, Message> {
        let mut content = widget::column([]).width(Length::Fill).spacing(12)
            .push(widget::row([
                action_button("System", Message::ScopeChanged(Scope::System), self.quadlets_only || self.scope != Scope::System),
                action_button("User", Message::ScopeChanged(Scope::User), self.quadlets_only || self.scope != Scope::User),
                action_button("Quadlets", Message::OpenQuadlets, true),
                widget::text(if self.quadlets_only { "User Quadlets".to_owned() } else { format!("{} units", self.scope.label()) }).into(),
            ]).align_y(Alignment::Center).spacing(8))
            .push(widget::search_input(
                if self.quadlets_only { "Search Quadlet path, service name or description" } else { "Search unit name, description or Quadlet path" },
                &self.search,
            ).on_input(Message::SearchChanged))
            .push(widget::text("Pins only change this applet's settings. Pinning does not start, stop, enable, or disable a unit.").size(12));
        if self.quadlets_only {
            if let Some(directory) = &self.quadlet_directory {
                content = content.push(
                    widget::text(format!("Quadlet directory: {}", directory.display())).size(12),
                );
            }
            content = content.push(widget::text("Sources are linked only by systemd SourcePath metadata. This applet does not edit sources or reload the user manager.").size(12));
            if let Some(error) = &self.quadlet_error {
                content = content.push(colored_text(
                    format!("Quadlet discovery or service mapping incomplete: {error}"),
                    Tone::Error,
                ));
            }
        }
        if self.catalog_loading {
            content = content.push(spinner(if self.quadlets_only {
                "Discovering Quadlet sources and generated services…"
            } else {
                "Loading unit catalog…"
            }));
        } else if let Some(error) = &self.catalog_error {
            content = content.push(colored_text(
                format!("Catalog unavailable: {error}"),
                Tone::Error,
            ));
        } else {
            content = content.push(pagination(self.catalog_page, self.filtered.len(), true));
            if self.filtered.is_empty() {
                content = content.push(widget::text(match (self.quadlets_only, self.search.trim().is_empty()) {
                    (true, true) => "No Quadlet sources were found in your user configuration directory. Use Refresh to discover changes.",
                    (true, false) => "No matching Quadlet sources or generated services. Change the search to see other results.",
                    (false, true) => "No units were returned for this scope. Use Refresh to try again.",
                    (false, false) => "No matching units. Change the search to see other results.",
                }));
            }
            let pin_enabled = self.config_loaded
                && !self.config_loading
                && self.pending_pins.is_none()
                && self.busy.is_none();
            for index in self
                .filtered
                .iter()
                .skip(self.catalog_page * PAGE_SIZE)
                .take(PAGE_SIZE)
            {
                if self.quadlets_only {
                    let source = &self.quadlets[*index];
                    content = content.push(widget::text(format!(
                        "Quadlet source: {}",
                        source.path.display()
                    )));
                    if !source
                        .unit_indices
                        .iter()
                        .any(|index| self.catalog.get(*index).is_some())
                    {
                        content = content.push(colored_text(
                            "No verified generated service found. Check the Quadlet source and generator errors, run systemctl --user daemon-reload, then Refresh.".to_owned(),
                            Tone::Warning,
                        ));
                    }
                    for unit in source
                        .unit_indices
                        .iter()
                        .filter_map(|index| self.catalog.get(*index))
                    {
                        content = content.push(self.catalog_unit_view(unit, pin_enabled, true));
                    }
                } else {
                    content = content.push(self.catalog_unit_view(
                        &self.catalog[*index],
                        pin_enabled,
                        false,
                    ));
                }
                content = content.push(widget::divider::horizontal::default());
            }
        }
        content.into()
    }

    fn confirmation_view<'a>(&'a self, id: &'a UnitId, action: UnitAction) -> Element<'a, Message> {
        let mut content = widget::column([])
            .width(Length::Fill)
            .spacing(12)
            .push(widget::text::title4(format!(
                "{} {}?",
                action.label(),
                id.name
            )))
            .push(widget::text(format!("Scope: {}", id.scope.label())));
        let warning = match id.scope {
            Scope::System => {
                "This changes the system manager. Stopping or restarting units can disrupt the desktop, networking, or other users."
            }
            Scope::User => {
                "This changes your user manager. Stopping or restarting units can disrupt your desktop session."
            }
        };
        content = content.push(colored_text(warning.to_owned(), Tone::Warning));
        content = content.push(widget::text(match action {
            UnitAction::Start => "Start requests activation now. It does not enable the unit.",
            UnitAction::Restart => "Restart stops and starts the unit. If it is inactive, restart starts it.",
            UnitAction::Stop => "Stop requests deactivation now. It does not disable the unit.",
            UnitAction::Enable => "Enable changes automatic activation links. It does not start the unit.",
            UnitAction::Disable => "Disable removes automatic activation links. It does not stop the unit or prevent manual activation.",
        }));
        if id.scope == Scope::System {
            content = content.push(widget::text("System changes use the standard Polkit authorization dialog when required. This applet does not collect passwords.").size(12));
        }
        content.push(widget::text("Cancel is selected by default. No change is made until you explicitly confirm.").size(12)).into()
    }

    fn footer(&self) -> Element<'_, Message> {
        if let Some((id, action)) = &self.confirmation {
            let cancel = widget::button::suggested("Cancel")
                .id(self.cancel_id.clone())
                .on_press(Message::CancelAction);
            let mut confirm = widget::button::destructive(format!("Confirm {}", action.label()));
            if self.action_allowed(id, *action) {
                confirm = confirm.on_press(Message::ConfirmAction);
            }
            return widget::row([
                action_button("Back", Message::CancelAction, true),
                widget::Space::new().width(Length::Fill).into(),
                cancel.into(),
                confirm.into(),
            ])
            .spacing(8)
            .align_y(Alignment::Center)
            .into();
        }
        widget::row([
            action_button(
                if self.settings { "Back" } else { "Settings" },
                if self.settings {
                    Message::Back
                } else {
                    Message::OpenSettings
                },
                true,
            ),
            widget::Space::new().width(Length::Fill).into(),
            widget::text(format!("{} pinned", self.pins.len()))
                .size(12)
                .into(),
        ])
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "io.github.cosmic-applet-systemd";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, _flags: ()) -> (Self, Task<cosmic::Action<Message>>) {
        let mut app = Self::new(core);
        let task = app.load_config();
        (app, task)
    }

    fn view(&self) -> Element<'_, Message> {
        let applet = &self.core.applet;
        applet
            .applet_tooltip(
                applet
                    .icon_button_from_handle(self.icon.clone())
                    .on_press(Message::TogglePopup),
                "Systemd Units",
                self.popup.is_some(),
                Message::SurfaceAction,
                self.core.main_window_id(),
            )
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        let title = if self.confirmation.is_some() {
            "Confirm unit action"
        } else if self.settings {
            "Settings"
        } else {
            "Systemd Units"
        };
        let header = widget::row([
            widget::text::title3(title).width(Length::Fill).into(),
            if self.status_loading {
                spinner("Refreshing…")
            } else {
                widget::Space::new().into()
            },
            action_button(
                "Refresh",
                Message::Refresh,
                self.busy.is_none()
                    && self.confirmation.is_none()
                    && !self.status_loading
                    && !self.catalog_loading,
            ),
        ])
        .align_y(Alignment::Center)
        .spacing(8);
        let mut body = widget::column([]).width(Length::Fill).spacing(12);
        if self.config_loading {
            body = body.push(spinner("Loading saved pins…"));
        }
        if let Some(error) = &self.config_error {
            body = body.push(colored_text(error.as_str(), Tone::Error));
            if !self.config_loaded {
                body = body.push(widget::text("Pin changes are blocked until saved settings can be read. The existing configuration will not be overwritten.").size(12))
                    .push(action_button("Retry loading settings", Message::RetryConfig, !self.config_loading));
            }
        }
        if self.pending_pins.is_some() {
            body = body.push(spinner(
                "Saving pins… Previous pins remain in use until saving succeeds.",
            ));
        }
        if let Some((id, action)) = &self.busy {
            body = body.push(spinner(format!(
                "{} {} ({} scope)…",
                action.label(),
                id.name,
                id.scope.label()
            )));
        }
        if let Some((notice, error)) = &self.notice {
            body = body.push(colored_text(
                notice.as_str(),
                if *error { Tone::Error } else { Tone::Success },
            ));
        }
        body = body.push(if let Some((id, action)) = &self.confirmation {
            self.confirmation_view(id, *action)
        } else if self.settings {
            self.settings_view()
        } else {
            self.pins_view()
        });
        let content = widget::column([])
            .width(Length::Fill)
            .spacing(12)
            .push(header)
            .push(
                widget::scrollable(
                    widget::container(body)
                        .padding(cosmic::iced::Padding {
                            top: 0.0,
                            right: 16.0,
                            bottom: 0.0,
                            left: 0.0,
                        })
                        .width(Length::Fill),
                )
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .push(widget::divider::horizontal::default())
            .push(self.footer());
        self.core
            .applet
            .popup_container(
                widget::container(content)
                    .padding(16)
                    .width(Length::Fill)
                    .height(Length::Fixed(600.0)),
            )
            .limits(
                Limits::NONE
                    .min_width(POPUP_WIDTH)
                    .max_width(POPUP_WIDTH)
                    .max_height(640.0),
            )
            .into()
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn on_escape(&mut self) -> Task<cosmic::Action<Message>> {
        if self.confirmation.is_some() {
            Task::done(cosmic::Action::App(Message::CancelAction))
        } else if self.popup.is_some() {
            Task::done(cosmic::Action::App(Message::TogglePopup))
        } else {
            Task::none()
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.popup.is_some()
            && self.busy.is_none()
            && self.confirmation.is_none()
            && !self.pins.is_empty()
        {
            cosmic::iced::time::every(Duration::from_secs(5 * 60)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        }
    }

    fn update(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::SurfaceAction(action) => return cosmic::surface::surface_task(action),
            Message::TogglePopup => {
                if let Some(id) = self.popup.take() {
                    self.confirmation = None;
                    return destroy_popup(id);
                }
                let Some(parent) = self.core.main_window_id() else {
                    return Task::none();
                };
                let id = Id::unique();
                self.popup = Some(id);
                self.invalidate_statuses();
                let mut settings = self
                    .core
                    .applet
                    .get_popup_settings(parent, id, None, None, None);
                settings.positioner.size_limits = Limits::NONE
                    .min_width(POPUP_WIDTH)
                    .max_width(POPUP_WIDTH)
                    .max_height(640.0);
                let catalog = if self.settings {
                    self.fetch_catalog()
                } else {
                    Task::none()
                };
                return Task::batch([get_popup(settings), self.fetch_statuses(), catalog]);
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                    self.confirmation = None;
                }
            }
            Message::Tick => {
                if self.popup.is_some() && self.busy.is_none() && self.confirmation.is_none() {
                    return self.fetch_statuses();
                }
            }
            Message::Refresh => {
                if self.busy.is_none() && self.confirmation.is_none() {
                    let catalog = if self.settings {
                        self.fetch_catalog()
                    } else {
                        Task::none()
                    };
                    return Task::batch([self.fetch_statuses(), catalog]);
                }
            }
            Message::OpenSettings => {
                self.confirmation = None;
                self.settings = true;
                return self.fetch_catalog();
            }
            Message::Back => {
                self.confirmation = None;
                self.settings = false;
            }
            Message::OpenQuadlets => {
                self.confirmation = None;
                self.settings = true;
                self.quadlets_only = true;
                self.scope = Scope::User;
                return self.fetch_catalog();
            }
            Message::ScopeChanged(scope) => {
                self.scope = scope;
                self.quadlets_only = false;
                self.catalog_page = 0;
                return self.fetch_catalog();
            }
            Message::SearchChanged(search) => {
                self.search = search;
                self.catalog_page = 0;
                self.filter_catalog();
            }
            Message::CatalogPage(page) => {
                self.catalog_page = page.min(last_page(self.filtered.len()));
            }
            Message::PinPage(page) => {
                self.pin_page = page.min(last_page(self.pins.len()));
            }
            Message::RetryConfig => return self.load_config(),
            Message::ConfigLoaded(generation, result) => {
                if generation != self.config_generation || !self.config_loading {
                    return Task::none();
                }
                self.config_loading = false;
                match result {
                    Ok(pins) => {
                        self.pins = pins;
                        self.config_loaded = true;
                        self.config_error = None;
                        self.pin_page = self.pin_page.min(last_page(self.pins.len()));
                        self.invalidate_statuses();
                        if self.popup.is_some() {
                            return self.fetch_statuses();
                        }
                    }
                    Err(error) => {
                        self.config_loaded = false;
                        self.config_error =
                            Some(format!("Saved pins could not be loaded: {error}"));
                    }
                }
            }
            Message::TogglePin(id) => {
                if self.stage_pin_change(id) {
                    let generation = self.config_generation;
                    let Some(pins) = self.pending_pins.clone() else {
                        return Task::none();
                    };
                    return Task::perform(
                        async move { config::save(&pins).await },
                        move |result| cosmic::Action::App(Message::PinsSaved(generation, result)),
                    );
                }
            }
            Message::PinsSaved(generation, result) => {
                if self.finish_pin_save(generation, result) {
                    let statuses = if self.popup.is_some() {
                        self.fetch_statuses()
                    } else {
                        Task::none()
                    };
                    return statuses;
                }
            }
            Message::CatalogFetched(scope, generation, result) => {
                if generation != self.catalog_generation
                    || scope != self.scope
                    || !self.catalog_loading
                {
                    return Task::none();
                }
                self.catalog_loading = false;
                match result {
                    Ok(catalog) => {
                        self.catalog = catalog.units;
                        self.quadlets = catalog.quadlets;
                        self.quadlet_directory = catalog.quadlet_directory;
                        self.quadlet_error = catalog.quadlet_error;
                        self.catalog_error = None;
                        self.filter_catalog();
                    }
                    Err(error) => {
                        self.catalog.clear();
                        self.filtered.clear();
                        self.catalog_error = Some(error);
                    }
                }
            }
            Message::StatusesFetched(generation, result) => self.apply_statuses(generation, result),
            Message::RequestAction(id, action) => {
                if self.confirmation.is_none() && self.action_allowed(&id, action) {
                    self.confirmation = Some((id, action));
                    return widget::button::focus(self.cancel_id.clone());
                }
            }
            Message::CancelAction => self.confirmation = None,
            Message::ConfirmAction => {
                let Some((id, action)) = self.confirmation.take() else {
                    return Task::none();
                };
                if !self.action_allowed(&id, action) {
                    self.notice = Some(("The unit's status changed or is unavailable. Refresh and review the action again; no change was made.".into(), true));
                    return Task::none();
                }
                self.action_generation += 1;
                let generation = self.action_generation;
                self.busy = Some((id.clone(), action));
                self.notice = None;
                self.invalidate_statuses();
                self.invalidate_catalog();
                let finished_id = id.clone();
                return Task::perform(backend::perform(id, action), move |result| {
                    cosmic::Action::App(Message::ActionFinished(
                        generation,
                        finished_id,
                        action,
                        result,
                    ))
                });
            }
            Message::ActionFinished(generation, id, action, result) => {
                if generation != self.action_generation
                    || !matches!(&self.busy, Some((busy_id, busy_action)) if busy_id == &id && *busy_action == action)
                {
                    return Task::none();
                }
                self.busy = None;
                self.notice = Some(match result {
                    Ok(result) => (
                        format!(
                            "{} {} ({} scope): {result}",
                            action.label(),
                            id.name,
                            id.scope.label()
                        ),
                        false,
                    ),
                    Err(error) => (
                        format!(
                            "{} {} ({} scope) failed: {error}",
                            action.label(),
                            id.name,
                            id.scope.label()
                        ),
                        true,
                    ),
                });
                self.invalidate_statuses();
                self.invalidate_catalog();
                let catalog = if self.settings {
                    self.fetch_catalog()
                } else {
                    Task::none()
                };
                // Completion always re-reads actual state, even if the popup was closed meanwhile.
                return Task::batch([self.fetch_statuses(), catalog]);
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

fn quadlet_enablement_guidance(scope: Scope) -> String {
    let reload = match scope {
        Scope::System => "systemctl daemon-reload",
        Scope::User => "systemctl --user daemon-reload",
    };
    format!(
        "Generator-managed service: Enable/Disable are unavailable. Configure automatic activation in the Quadlet source's [Install] section; after changing it, run {reload}, then Refresh."
    )
}

fn unit_matches(unit: &UnitListing, needle: &str) -> bool {
    needle.is_empty()
        || unit.id.name.to_lowercase().contains(needle)
        || unit.description.to_lowercase().contains(needle)
        || unit
            .quadlet_source
            .as_ref()
            .is_some_and(|source| source.to_string_lossy().to_lowercase().contains(needle))
}

fn last_page(count: usize) -> usize {
    count.saturating_sub(1) / PAGE_SIZE
}

fn pagination(page: usize, count: usize, catalog: bool) -> Element<'static, Message> {
    let first = if count == 0 { 0 } else { page * PAGE_SIZE + 1 };
    let last = ((page + 1) * PAGE_SIZE).min(count);
    let previous = if catalog {
        Message::CatalogPage(page.saturating_sub(1))
    } else {
        Message::PinPage(page.saturating_sub(1))
    };
    let next = if catalog {
        Message::CatalogPage(page + 1)
    } else {
        Message::PinPage(page + 1)
    };
    widget::row([
        widget::text(format!("{first}–{last} of {count}"))
            .size(12)
            .width(Length::Fill)
            .into(),
        action_button("Previous", previous, page > 0),
        action_button("Next", next, page < last_page(count)),
    ])
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn action_button<'a>(label: &'a str, message: Message, enabled: bool) -> Element<'a, Message> {
    widget::button::text(label)
        .on_press_maybe(enabled.then_some(message))
        .into()
}

fn spinner<'a>(label: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Element<'a, Message> {
    widget::row([
        widget::progress_bar::indeterminate_circular()
            .size(16.0)
            .into(),
        widget::text(label).size(12).into(),
    ])
    .align_y(Alignment::Center)
    .spacing(8)
    .into()
}

#[derive(Clone, Copy)]
enum Tone {
    Neutral,
    Success,
    Warning,
    Error,
    Accent,
}

fn colored_text<'a>(
    label: impl Into<std::borrow::Cow<'a, str>> + 'a,
    tone: Tone,
) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let color = match tone {
        Tone::Neutral => theme.cosmic().on_bg_color(),
        Tone::Success => theme.cosmic().success_color(),
        Tone::Warning => theme.cosmic().warning_color(),
        Tone::Error => theme.cosmic().destructive_color(),
        Tone::Accent => theme.cosmic().accent_color(),
    };
    widget::text(label)
        .size(12)
        .class(cosmic::theme::Text::Color(color.into()))
        .into()
}

fn indicators<'a>(active: &str, sub: &str, file: &str) -> Element<'a, Message> {
    let activity_tone = match active {
        "active" => Tone::Success,
        "failed" => Tone::Error,
        "activating" | "deactivating" | "reloading" | "refreshing" => Tone::Warning,
        _ => Tone::Neutral,
    };
    let file_tone = match file {
        "enabled" | "enabled-runtime" => Tone::Success,
        "masked" | "masked-runtime" | "bad" => Tone::Error,
        "static" | "alias" | "generated" | "transient" | "indirect" | "linked"
        | "linked-runtime" => Tone::Accent,
        _ => Tone::Neutral,
    };
    widget::column([])
        .spacing(3)
        .push(colored_text(
            format!("Activity: {}", activity_label(active, sub)),
            activity_tone,
        ))
        .push(colored_text(
            format!("Enablement: {}", file_label(file)),
            file_tone,
        ))
        .into()
}

fn activity_label(active: &str, sub: &str) -> String {
    let label = match active {
        "active" if sub == "running" => return "Running".into(),
        "active" => "Active",
        "failed" => "Failed",
        "activating" => "Activating",
        "deactivating" => "Deactivating",
        "inactive" => "Inactive",
        "reloading" => "Reloading",
        "refreshing" => "Refreshing",
        "" | "unknown" => return "Unknown".into(),
        other => other,
    };
    if sub.is_empty() || sub == active {
        label.into()
    } else {
        format!("{label} ({sub})")
    }
}

fn file_label(state: &str) -> &str {
    match state {
        "enabled" => "Enabled",
        "enabled-runtime" => "Enabled (runtime only)",
        "disabled" => "Disabled",
        "masked" => "Masked",
        "masked-runtime" => "Masked (runtime only)",
        "static" => "Static",
        "alias" => "Alias",
        "generated" => "Generated",
        "transient" => "Transient",
        "indirect" => "Indirect",
        "linked" => "Linked",
        "linked-runtime" => "Linked (runtime only)",
        "bad" => "Invalid unit file",
        "" | "unknown" => "Unknown",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(scope: Scope, name: &str) -> UnitId {
        UnitId {
            scope,
            name: name.into(),
        }
    }

    fn active_state() -> UnitState {
        UnitState {
            description: "Test unit".into(),
            load_state: "loaded".into(),
            active_state: "active".into(),
            sub_state: "running".into(),
            unit_file_state: "disabled".into(),
            quadlet_source: None,
            can_start: true,
            can_restart: true,
            can_stop: true,
            can_enable: true,
            can_disable: true,
        }
    }

    fn listing(name: &str) -> UnitListing {
        UnitListing {
            id: id(Scope::User, name),
            description: "Test unit".into(),
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            unit_file_state: "generated".into(),
            quadlet_source: None,
        }
    }

    fn quadlet_catalog() -> UnitCatalog {
        let source = PathBuf::from("/config/containers/systemd/engine.container");
        let mut generated = listing("z-overridden.service");
        generated.description = "Container with overridden service name".into();
        generated.quadlet_source = Some(source.clone());
        UnitCatalog {
            units: vec![generated, listing("a-unmatched.service")],
            quadlets: vec![
                QuadletFile {
                    path: source,
                    unit_indices: vec![0],
                },
                QuadletFile {
                    path: PathBuf::from("/config/containers/systemd/a-unmatched.container"),
                    unit_indices: Vec::new(),
                },
            ],
            quadlet_directory: Some("/config/containers/systemd".into()),
            quadlet_error: Some("One source could not be mapped".into()),
        }
    }

    #[test]
    fn catalog_display_sort_preserves_verified_quadlet_indices() {
        let mut app = AppModel::new(cosmic::Core::default());
        app.scope = Scope::User;
        app.catalog_loading = true;
        let generation = app.catalog_generation;
        let _ = cosmic::Application::update(
            &mut app,
            Message::CatalogFetched(Scope::User, generation, Ok(quadlet_catalog())),
        );
        assert_eq!(app.filtered, vec![1, 0]);
        assert_eq!(
            app.catalog[app.quadlets[0].unit_indices[0]].id.name,
            "z-overridden.service"
        );
        assert!(app.quadlets[1].unit_indices.is_empty());
        assert!(app.quadlet_error.is_some());
    }

    #[test]
    fn quadlet_search_matches_source_paths_and_verified_service_metadata() {
        let mut app = AppModel::new(cosmic::Core::default());
        app.quadlets_only = true;
        let catalog = quadlet_catalog();
        app.catalog = catalog.units;
        app.quadlets = catalog.quadlets;
        for search in [
            "engine.container",
            "/config/containers/systemd/engine",
            "Z-OVERRIDDEN.SERVICE",
            "overridden service name",
        ] {
            app.search = search.into();
            app.filter_catalog();
            assert_eq!(app.filtered, vec![0], "Search: {search}");
        }
        app.search = "a-unmatched.container".into();
        app.filter_catalog();
        assert_eq!(app.filtered, vec![1]);
        assert!(app.quadlets[app.filtered[0]].unit_indices.is_empty());
    }

    #[test]
    fn quadlet_activation_refreshes_user_catalog_and_rejects_stale_data() {
        let mut app = AppModel::new(cosmic::Core::default());
        let stale_generation = app.catalog_generation;
        let _ = cosmic::Application::update(&mut app, Message::OpenQuadlets);
        assert_eq!(app.scope, Scope::User);
        assert!(app.settings && app.quadlets_only && app.catalog_loading);
        let _ = cosmic::Application::update(
            &mut app,
            Message::CatalogFetched(Scope::User, stale_generation, Ok(quadlet_catalog())),
        );
        assert!(app.catalog.is_empty() && app.quadlets.is_empty());
        let generation = app.catalog_generation;
        let _ = cosmic::Application::update(
            &mut app,
            Message::CatalogFetched(Scope::User, generation, Ok(quadlet_catalog())),
        );
        let _ = cosmic::Application::update(&mut app, Message::ScopeChanged(Scope::System));
        assert!(!app.quadlets_only);
        assert!(app.catalog.is_empty() && app.quadlets.is_empty());
        assert!(app.quadlet_directory.is_none() && app.quadlet_error.is_none());
    }

    #[test]
    fn failed_pin_save_keeps_last_good_pins() {
        let mut app = AppModel::new(cosmic::Core::default());
        let previous = id(Scope::User, "previous.service");
        app.config_loaded = true;
        app.pins = vec![previous.clone()];
        app.stage_pin_change(id(Scope::System, "new.service"));
        app.finish_pin_save(app.config_generation, Err("disk full".into()));
        assert_eq!(app.pins, vec![previous]);
    }

    #[test]
    fn pin_saves_are_serialized_and_preserve_scope_and_order() {
        let mut app = AppModel::new(cosmic::Core::default());
        let user = id(Scope::User, "same.service");
        let system = id(Scope::System, "same.service");
        app.config_loaded = true;
        app.pins = vec![user.clone()];
        app.stage_pin_change(system.clone());
        app.stage_pin_change(id(Scope::User, "ignored.service"));
        app.finish_pin_save(app.config_generation, Ok(()));
        assert_eq!(app.pins, vec![user, system]);
    }

    #[test]
    fn stale_status_response_cannot_restore_pre_mutation_state() {
        let mut app = AppModel::new(cosmic::Core::default());
        let unit = id(Scope::User, "test.service");
        app.pins.push(unit.clone());
        app.status_loading = true;
        let old_generation = app.status_generation;
        app.invalidate_statuses();
        app.apply_statuses(old_generation, vec![(unit, Ok(active_state()))]);
        assert!(app.states.is_empty());
    }

    #[test]
    fn failed_status_read_removes_mutation_permission() {
        let mut app = AppModel::new(cosmic::Core::default());
        let unit = id(Scope::User, "test.service");
        app.pins.push(unit.clone());
        app.states.insert(unit.clone(), Ok(active_state()));
        app.status_loading = true;
        app.apply_statuses(
            app.status_generation,
            vec![(unit.clone(), Err("bus unavailable".into()))],
        );
        assert!(!app.action_allowed(&unit, UnitAction::Stop));
    }

    #[test]
    fn confirmation_rechecks_status_before_dispatching_action() {
        let mut app = AppModel::new(cosmic::Core::default());
        let unit = id(Scope::User, "test.service");
        app.pins.push(unit.clone());
        app.states.insert(unit.clone(), Ok(active_state()));
        app.confirmation = Some((unit.clone(), UnitAction::Stop));
        app.states.insert(unit, Err("bus disconnected".into()));
        let _ = cosmic::Application::update(&mut app, Message::ConfirmAction);
        assert!(app.busy.is_none());
    }

    #[test]
    fn config_load_failure_blocks_saving_without_erasing_existing_pins() {
        let mut app = AppModel::new(cosmic::Core::default());
        let previous = id(Scope::User, "previous.service");
        app.pins.push(previous.clone());
        app.config_loading = true;
        let generation = app.config_generation;
        let _ = cosmic::Application::update(
            &mut app,
            Message::ConfigLoaded(generation, Err("invalid configuration".into())),
        );
        app.stage_pin_change(id(Scope::User, "new.service"));
        assert_eq!((app.pins, app.pending_pins), (vec![previous], None));
    }

    #[test]
    fn status_refresh_preserves_search_focus() {
        use cosmic::Application;
        use cosmic::iced::advanced::widget::{Tree, tree};

        fn search_state(tree: &mut Tree) -> Option<&mut widget::text_input::State> {
            if tree.tag == tree::Tag::of::<widget::text_input::State>() {
                Some(tree.state.downcast_mut())
            } else {
                tree.children.iter_mut().find_map(search_state)
            }
        }

        let mut app = AppModel::new(cosmic::Core::default());
        let popup = Id::unique();
        app.popup = Some(popup);
        app.settings = true;
        app.search = "ssh".into();
        let unit = id(Scope::User, "test.service");
        app.pins.push(unit.clone());
        let mut tree = Tree::new(app.view_window(popup).as_widget());
        search_state(&mut tree).unwrap().focus();

        let _ = app.update(Message::Tick);
        assert!(app.status_loading);
        tree.diff(app.view_window(popup));
        assert!(search_state(&mut tree).unwrap().is_focused());

        let _ = app.update(Message::StatusesFetched(
            app.status_generation,
            vec![(unit, Ok(active_state()))],
        ));
        tree.diff(app.view_window(popup));
        assert!(search_state(&mut tree).unwrap().is_focused());
    }
}
