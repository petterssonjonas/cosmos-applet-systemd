# COSMIC Systemd Units Applet

A native COSMIC panel applet for inspecting and controlling pinned systemd units. The panel button uses a theme-aware controls icon with the tooltip **Systemd Units**.

## Usage

Click the panel icon to open the dropdown. Pins are empty on first launch; nothing is selected automatically. Open **Settings** at the bottom, choose **System** or **User**, search unit names or descriptions, and select **Pin**. Pins retain their order across restarts. **Unpin** removes a unit from this applet without changing systemd.

Each pinned unit shows its scope, description, and separate indicators:

- **Activity**: running, inactive, failed, activating, or another state reported by systemd. An active oneshot with no running process is shown as **Active (exited)**, not Running.
- **Enablement**: enabled, disabled, static, masked, runtime-enabled, or another reported unit-file state. Enabled does not mean running; disabled does not mean stopped.

All five controls are displayed. Unsupported actions are disabled with an explanation:

| Action | Effect |
| --- | --- |
| Start | Activate now without enabling automatic activation. |
| Restart | Restart now without changing enablement. systemd may start an inactive unit. |
| Stop | Deactivate now without disabling automatic activation. |
| Enable | Create persistent activation links without starting the unit or overriding a mask. |
| Disable | Remove local persistent and runtime activation links without stopping the unit or masking it. |

Every mutation requires confirmation; **Cancel** receives default focus. System units may affect networking, the desktop, or other users. Read the selected unit name and scope before confirming, especially for targets and mounts.

Pinned statuses refresh **when the dropdown opens**, every **5 minutes while it remains open**, after operations, and on **Refresh**. There is no status polling while closed. The status spinner stays in the header so refreshes preserve search focus. Settings fetch the unit catalog when opened, when switching scope, and on **Refresh**. Search results and pins are paginated; scroll inside the dropdown for additional controls. Settings remain accessible in the fixed footer.

## Podman Quadlets

Open **Settings → Quadlets** to discover files under `$XDG_CONFIG_HOME/containers/systemd`, normally `~/.config/containers/systemd`. Discovery includes subdirectories and symlinks, with cycle protection. Recognized source types are `.container`, `.pod`, `.kube`, `.network`, `.volume`, `.image`, `.build`, and `.artifact`.

Podman's generator turns Quadlets into ordinary systemd services during manager startup or `daemon-reload`. For example, `open-webui.container` normally produces the user unit `open-webui.service`. The applet matches actual services using systemd's `SourcePath`, not `ContainerName` or a guessed filename, so custom service names and aliases can be matched. Search accepts source paths, service names, and descriptions.

Pin a generated service to use **Start**, **Restart**, and **Stop** through the existing systemd controls. Pinned cards show the Quadlet source path and independent activity/enablement state. Saved pins still contain only the actual unit name and scope.

**Generated services cannot be enabled or disabled like ordinary unit files.** Their automatic activation comes from `[Install]` in the Quadlet source or its drop-ins, for example:

```ini
[Install]
WantedBy=default.target
```

After changing a user Quadlet, run `systemctl --user daemon-reload`, then **Refresh**. The applet does not edit Quadlet files, execute the generator, or reload the manager merely to discover sources. The five-minute status refresh policy is unchanged.

Sources without a verified generated service remain visible with guidance, but no Pin button targeting a guessed service. Discovery/mapping failures are reported without discarding the ordinary unit catalog. Template definitions cannot be started directly; select a concrete instance. Template source paths requiring unresolved escaping or specifier expansion are reported as unverifiable rather than guessed.

The dedicated Quadlets view inventories the user's configuration directory. Quadlet-backed units from other generator search paths remain accessible through the regular System/User lists; their source metadata is shown when available.

## Authorization and boundaries

The applet talks directly to the system or session systemd manager over D-Bus. It does not run shell commands, run as root, invoke sudo, or collect passwords. System mutations allow native **Polkit** authorization; the desktop authentication agent supplies any required prompt. A missing authentication agent or denied authorization is shown as an error.

Capabilities are rechecked before each operation. Start, Restart, and Stop wait for the matching systemd job to finish; failed jobs are not reported as success. Timeouts do not imply cancellation: refresh status before retrying, since a job may continue.

Enable and Disable reload the selected manager after link changes and report the resulting unit-file state. A user unit enabled through global configuration may remain enabled after its local links are removed; the applet reports this rather than silently masking it. Static/generated units, masks, aliases, and template definitions retain systemd's restrictions. Pin a concrete template instance to start it. Disabling automatic activation does not prevent dependencies or other activation sources from starting a unit.

The catalog combines loaded units and installed unit files. An unloaded file can have an unknown description/activity until its status is queried. Pinning queries metadata but does not start or enable the unit. Unknown or unavailable data is never presented as a confirmed stopped/disabled state.

No external network service, account, or credential storage is required.

## Build and install

Requirements: a current Rust toolchain, the [libcosmic build dependencies](https://github.com/pop-os/libcosmic), `just`, `desktop-file-utils`, a COSMIC desktop session, and systemd with accessible system/session D-Bus managers. System changes also need a working desktop Polkit authentication agent.

```sh
cargo build --release --locked
just install
```

The default prefix is `~/.local`. Installation writes the executable, desktop entry, and icon there. The desktop entry uses an absolute executable path, so the panel does not need `~/.local/bin` in its `PATH`.

Add **Systemd Units** in **COSMIC Settings → Desktop → Panel → Configure panel applets**. The desktop ID is `io.github.cosmic-applet-systemd`. `just install` itself does not alter the panel layout. Remove and re-add the applet to load an updated executable.

```sh
cargo test --release --locked
cargo clippy --release --all-targets --locked -- -D warnings
```

## Saved pins

Pins are stored in `$XDG_CONFIG_HOME/cosmic-applet-systemd/pins.json`, normally `~/.config/cosmic-applet-systemd/pins.json`. System and user units with the same name are separate entries. Changes use private files and atomic replacement; the UI retains its previous pins if saving fails. Invalid or unsupported configuration is reported rather than overwritten with an empty list.

To uninstall, remove the applet from the panel, then run `just uninstall`. Saved pins remain available for a later installation.

## Verification

Verified locally on Fedora 44, systemd 259, and COSMIC:

- Release build, 32 tests, strict Clippy, and desktop-entry validation. A regression test verifies that search focus survives both the beginning and completion of a status refresh.
- Real system and user unit catalog reads.
- Start, Restart, Stop, Enable, and Disable against disposable user services. Restart produced a new process; Enable/Disable did not change runtime activity.
- Removal of runtime-only and combined persistent/runtime enablement.
- Failed-job reporting and active/exited oneshot state handling.
- Native popup rendering, empty first launch, searchable settings, pin persistence/removal, independent indicators, default-Cancel confirmation, and a confirmed UI action starting a disposable service.
- Read-only Quadlet discovery against four real local sources and their generated services; isolated checks for symlink cycles and an unmatched `dbus.container` that must not control the unrelated `dbus.service`.
- Native Quadlets selection, source search, pin saving, and pin restoration were exercised through the running application. Desktop screenshot capture timed out in this session, so the new Quadlets layout was not visually captured.

No existing user or system unit was started, stopped, restarted, enabled, or disabled during verification. System-scope mutation and its interactive Polkit prompt were not exercised. Temporary smoke drivers, pins, and unit fixtures are not shipped.

Implementation follows the [COSMIC applet template](https://github.com/pop-os/cosmic-applet-template), the [systemd D-Bus interface](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.systemd1.html), and the [Podman Quadlet documentation](https://docs.podman.io/en/latest/markdown/podman-systemd.unit.5.html).
