# Cosmos Systemd

Inspect and control pinned systemd units from the COSMIC panel. Keep system units and user units separate.
Cosmos is independent of System76. COSMIC libraries and protocols keep their upstream names.

## Requirements

Use Linux with a COSMIC desktop session and accessible system/session systemd managers over D-Bus.
System changes need a working desktop Polkit authentication agent.
Install native build packages for your distribution:

```sh
# Fedora
sudo dnf install gcc gcc-c++ cmake git pkgconf-pkg-config just desktop-file-utils expat-devel fontconfig-devel freetype-devel libxkbcommon-devel wayland-devel

# Debian 13 or newer
sudo apt update
sudo apt install build-essential cmake git pkgconf just desktop-file-utils libexpat1-dev libfontconfig-dev libfreetype-dev libxkbcommon-dev libwayland-dev

# Arch Linux
sudo pacman -Syu --needed base-devel cmake git pkgconf just desktop-file-utils expat fontconfig freetype2 libxkbcommon wayland
```

The lists follow [libcosmic's native requirements](https://github.com/pop-os/libcosmic#dependencies) and distribution package metadata.
They are package examples, not a claim that all three distributions were tested.
Install current stable Rust with [rustup](https://rustup.rs), as your normal user. Distribution Rust can be too old.
After rustup setup, open a new terminal and run:

```sh
rustup update stable
rustup default stable
```

## Build and install

Use `sudo` only for package prerequisites above, never for these build or install commands:

```sh
git clone https://github.com/petterssonjonas/cosmos-applet-systemd
cd cosmos-applet-systemd
just build-release
just install
```

Build and install are separate. Installation uses the existing release build.
The executable goes to `~/.local/bin/cosmos-applet-systemd`. The desktop entry and icon go to `~/.local/share`.
The desktop entry uses an absolute executable path. Keep `CARGO_TARGET_DIR` the same for both commands if you set it.
In a development container, use your host-mapped normal user, not root.

Open **COSMIC Settings → Desktop → Panel → Configure panel applets**.
Add **Cosmos Systemd** (`dev.jp.CosmosAppletSystemd`). Remove and add it again after an upgrade.
Installation does not change the panel layout.

## Use and authorization

Open the dropdown, then **Settings**. Select **System** or **User**, search for a unit, and select **Pin**.
No units are pinned automatically. **Unpin** removes only the saved pin; it does not change the unit.
Status refreshes when the dropdown opens, every five minutes while open, after operations, and on **Refresh**.
There is no status polling while closed.

Activity and enablement are independent. **Active (exited)** does not mean a process is running.
Enabled does not mean running; disabled does not mean stopped.

- **Start**, **Restart**, and **Stop** change current activity, not enablement. Restart can start an inactive unit.
- **Enable** adds activation links without starting the unit or overriding a mask.
- **Disable** removes local activation links without stopping or masking the unit. Global user links can still keep it enabled.

Every change requires confirmation. **Cancel** has default focus. System units can affect networking, the desktop, and other users.
Read the unit name and scope before confirming.

The applet uses D-Bus directly. It does not invoke a shell, run as root, invoke `sudo`, or collect passwords.
System changes can show a native Polkit prompt at runtime. This is separate from unprivileged applet installation.
Missing authorization or a missing authentication agent produces an error.
A timeout does not cancel a systemd job. Refresh status before retrying.
Static units, generated units, masks, aliases, and templates keep systemd's restrictions. Select a concrete template instance.
Unknown data is not shown as a confirmed stopped or disabled state. No network service or account is required.

### Podman Quadlets

Open **Settings → Quadlets** to find sources in `~/.config/containers/systemd`, following `$XDG_CONFIG_HOME` when set.
Podman and its Quadlet generator must already be installed and configured for Quadlet use.
The applet matches generated services through systemd's `SourcePath`; it never guesses a service name.
Unmatched sources remain visible but cannot be pinned as guessed services.

Use **Start**, **Restart**, or **Stop** on a verified generated service.
Generated services cannot use ordinary **Enable** or **Disable**. Set automatic activation in the Quadlet source instead:

```ini
[Install]
WantedBy=default.target
```

After editing a user Quadlet, run `systemctl --user daemon-reload`, then select **Refresh**.
The applet does not edit Quadlets, run the generator, or reload the manager for discovery.
Other generator search paths remain accessible through the normal unit lists.

## Saved pins and migration

Pins use `~/.config/cosmos-applet-systemd/pins.json`, or the corresponding `$XDG_CONFIG_HOME` directory.
Invalid configuration is reported rather than overwritten with an empty list.

To migrate, stop the old applet. Before launching the new applet, run `just migrate`.
It copies `~/.config/cosmic-applet-systemd` to `~/.config/cosmos-applet-systemd`, following `$XDG_CONFIG_HOME` when set.
Migration keeps the source, refuses an existing destination, and rejects symlinks or untrusted paths.
There is no automatic migration or old-name runtime fallback. Quadlet sources stay in `containers/systemd` and are not copied.

Replace the old **Systemd Units** panel entry with **Cosmos Systemd**.
Remove legacy files separately under the old prefix: `bin/cosmic-applet-systemd`,
`share/applications/io.github.cosmic-applet-systemd.desktop`, and
`share/icons/hicolor/scalable/apps/io.github.cosmic-applet-systemd.svg`.

## Uninstall

Remove the applet from the panel. Run `just uninstall` from this source directory, without `sudo`.
Uninstall keeps saved pins and legacy files. It does not change systemd units or Quadlet sources.

## License and references

Code: MPL-2.0, as declared in `Cargo.toml`.
Built with [libcosmic](https://github.com/pop-os/libcosmic) and the [COSMIC applet template](https://github.com/pop-os/cosmic-applet-template).
See the [systemd D-Bus interface](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.systemd1.html)
and [Podman Quadlet documentation](https://docs.podman.io/en/latest/markdown/podman-systemd.unit.5.html).
