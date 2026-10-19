import 'recipes/common.just'

name := 'cosmos-applet-systemd'
appid := 'dev.jp.CosmosAppletSystemd'
profile := 'release'
prefix := env('HOME') / '.local'
rootdir := ''
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
base-dir := absolute_path(clean(rootdir / prefix))

[default]
default: build-release

build-debug:
    cargo build --locked --target-dir {{quote(cargo-target-dir)}}

build-release:
    cargo build --release --locked --target-dir {{quote(cargo-target-dir)}}

check:
    cargo clippy --all-targets --locked --target-dir {{quote(cargo-target-dir)}} -- -D warnings

test:
    cargo test --locked --target-dir {{quote(cargo-target-dir)}}

# The installed applet needs a COSMIC panel; direct runs are for development.
[positional-arguments]
run *args:
    @printf '%s\n' 'Applet surfaces require a COSMIC session; add Cosmos Systemd through panel settings.'
    if [[ {{quote(profile)}} == debug ]]; then cargo run --locked --target-dir {{quote(cargo-target-dir)}} -- "$@"; else cargo run --locked --profile {{quote(profile)}} --target-dir {{quote(cargo-target-dir)}} -- "$@"; fi

# Build separately. Staging never changes the final desktop Exec path.
install:
    @just --justfile {{quote(justfile())}} _install-file {{quote(cargo-target-dir / profile / name)}} {{quote(base-dir / 'bin' / name)}} 0755
    @just --justfile {{quote(justfile())}} _install-desktop resources/app.desktop {{quote(base-dir / 'share/applications' / appid + '.desktop')}} {{quote(absolute_path(prefix / 'bin' / name))}}
    @just --justfile {{quote(justfile())}} _install-file resources/icon.svg {{quote(base-dir / 'share/icons/hicolor/scalable/apps' / appid + '.svg')}} 0644

# Files only: remove the panel entry yourself. Saved pins are retained.
uninstall:
    rm -f -- {{quote(base-dir / 'bin' / name)}} {{quote(base-dir / 'share/applications' / appid + '.desktop')}} {{quote(base-dir / 'share/icons/hicolor/scalable/apps' / appid + '.svg')}}

# Stop the old applet before copying its pins.
migrate:
    #!/usr/bin/env bash
    set -euo pipefail
    config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
    just --justfile {{quote(justfile())}} _migrate-directory "$config_home/cosmic-applet-systemd" "$config_home/cosmos-applet-systemd"
