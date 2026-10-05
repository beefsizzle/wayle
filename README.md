# beefsizzle/wayle

A bug-fix, robustness and lean-ness pass over [waltmck/wayle](https://github.com/waltmck/wayle), itself a fork of [wayle-rs/wayle](https://github.com/wayle-rs/wayle) (a GTK4/Rust Wayland shell: bar, dropdowns, notifications, OSD, wallpaper). The work targets a **niri** desktop and was done in October 2026 against niri 26.04, GTK 4.22 and Mesa 26.2.

`master` here is unchanged from `waltmck/wayle`. **Every change lives on its own topic branch**, one change per branch, so each can be reviewed, tested and upstreamed on its own. Nothing here has been merged upstream yet and no pull requests are open. The companion repository [beefsizzle/wayle-services](https://github.com/beefsizzle/wayle-services) holds the matching service-crate fixes (audio, systray, wallpaper, notifications, network, media).

> Most of this was written with Claude Code (Claude Sonnet and Opus); every commit carries a `Co-Authored-By` trailer, and the larger changes were reviewed by separate Opus passes. Each fix was checked against its symptom, usually in a nested niri session (see "How it was tested"), but it has been exercised on one machine only. Treat it as a well-tested proposal, not a release.

## What's on the branches

### Fixes and robustness (`beefsizzle/wayle`)

| Branch | What it fixes |
|---|---|
| `fix/radius-none-unit` | `button-rounding = "none"` produced invalid CSS (`--radius-none: 0` has no unit inside `calc()`), logging a GTK parse error on every start. |
| `fix/niri-workspace-map-schema` | niri's `workspace-map` schema collided with Hyprland's and rejected named workspaces ([#369](https://github.com/wayle-rs/wayle/issues/369)). |
| `fix/niri-hide-trailing-empty` | `hide-trailing-empty = false` had no effect, `min-workspace-count` does not apply to niri's dynamic workspaces, the active workspace could be hidden, and `empty-workspace-above-first` leaked through ([#199](https://github.com/wayle-rs/wayle/issues/199), [#272](https://github.com/wayle-rs/wayle/issues/272)). A stale `min-workspace-count` now warns instead of failing. |
| `fix/config-merge-custom-modules` | `[[modules.custom]]` entries from several imported files overwrote each other; they now concatenate, and a later entry with the same `id` wins ([#353](https://github.com/wayle-rs/wayle/issues/353)). |
| `fix/runtime-toml-concurrent-writes` | Concurrent `wayle config set` runs lost updates or corrupted `runtime.toml` ([#208](https://github.com/wayle-rs/wayle/issues/208)): per-writer temp files, a cross-process lock, and no stale echo-back from the settings app. |
| `fix/runtime-layer-skip-bad-field` | One invalid value in `runtime.toml` aborted loading its siblings, which were then dropped from disk on the next save. |
| `feat/warn-runtime-overrides` | One startup warning listing the keys where `runtime.toml` silently overrides `config.toml`. |
| `fix/dashboard-settings-binary-path` | The dashboard's settings button now launches the `wayle-settings` next to the running shell instead of whatever is on `$PATH`. |
| `fix/settings-single-close-button` | The settings window showed GTK's titlebar close button on top of its own. |

### Features

| Branch | What it adds |
|---|---|
| `feat/ext-background-effect` | `bar.background-blur` and `bar.dropdown-blur`: wayle requests compositor blur through the standard `ext-background-effect-v1` protocol with the *exact* visible shape of the bar and each dropdown card, so no niri `layer-rule` is needed and no padding or click-catching scrim gets blurred ([#350](https://github.com/wayle-rs/wayle/issues/350)). Off by default; only visible on translucent surfaces. |

### Performance (memory and threads)

| Branch | What it does |
|---|---|
| `perf/shell-lean-runtime` | Caps glibc malloc arenas at 2 and the tokio runtime at 4 workers (`MALLOC_ARENA_MAX` / `TOKIO_WORKER_THREADS` still override). |
| `perf/release-small-profile` | `cargo build --profile release-small` (`opt-level = "s"`): binary 61 to 46 MiB, about 10 MB less resident memory, no measurable idle-CPU change. The normal release profile is untouched. |

Measured in a nested niri session with a copy of a real config (release builds, one run each, about ±10 MB noise on the GL runs):

| Build | RSS at start | RSS after opening every dropdown | Threads |
|---|---|---|---|
| before | ~198 MB | 295 to 307 MB | 45 to 78 |
| both perf branches, default (GL) renderer | 182 to 189 MB | 256 to 263 MB | 34 to 36 at start |
| both perf branches, `GSK_RENDERER=vulkan` | ~150 MB | ~172 MB | 29 to 34 |

Most of the growth comes from GTK's GL renderer giving every popover its own GPU context with Mesa helper threads, not from wayle's own data. Wayle forces `GSK_RENDERER=gl` unless you set it; GTK 4.22's own default is Vulkan, which cut memory by about a third. Vulkan was tried on an AMD GPU only, and its behaviour on other drivers (NVIDIA in particular) is untested, so the default was not changed. Idle CPU was already about 0.05% and idle wakeups 4 to 10 per second, and the timers, pollers and unbounded channels that were measured showed nothing worth changing.

### Tests, lints and CI

| Branch | What it adds |
|---|---|
| `test/css-matrix` | A test that parses the generated stylesheet for every combination of the styling enum options (the full product of the four rounding options plus a pairwise cover of the rest, 681 combinations) with GTK's CSS parser, headless. On `master` it fails 240 combinations with the calc error above; with `fix/radius-none-unit` it passes. |
| `fix/clippy-baseline` | Fixes the two clippy errors that kept `cargo clippy --workspace --all-targets -- -D warnings` red. |
| `ci/fork-workflows` | A headless CSS-matrix CI job, release and docs workflows restricted to upstream, and an informational formatting check (the tree has never been nightly-formatted: 107 files differ). Includes the two branches above. The workflow has **not been run on GitHub yet**. |

### Cherry-picked from upstream pull requests

Original authorship is kept (`git cherry-pick -x`).

| Branch | Upstream PR |
|---|---|
| `pick/371-brightness-min` | [#371](https://github.com/wayle-rs/wayle/pull/371) brightness slider stops at 1% |
| `pick/294-calendar-week-start` | [#294](https://github.com/wayle-rs/wayle/pull/294) configurable calendar week start |
| `pick/291-systray-left-click` | [#291](https://github.com/wayle-rs/wayle/pull/291) left-click opens the menu for `ItemIsMenu` items (adapted to this fork's menu API) |
| `pick/289-settings-launcher-layoutcard` | [#289](https://github.com/wayle-rs/wayle/pull/289), the settings-launcher and layout-card parts only |
| `pick/351-icons-setup-runtime` | [#351](https://github.com/wayle-rs/wayle/pull/351) `wayle icons setup` works from release builds |

### Service crates (`beefsizzle/wayle-services`)

| Branch | What it fixes |
|---|---|
| `fix/audio-reconnect` | The audio service never recovered if PipeWire was not ready at start or restarted. It now reconnects with backoff, clears stale devices, and refuses commands from stale handles (after a restart pipewire reuses device indices, and a stale handle used to change a *different* device). Verified against a private PipeWire server with repeated kill/restart cycles. |
| `fix/systray-races` | The tray could stop accepting new items with no error: a burst of about 65 icon-change signals from one app wedged the shared D-Bus connection. Also ghost and duplicate icons for apps that register through a second connection (KDE apps). Verified with a stress harness on a private bus, including two 30-minute soaks. |
| `fix/wallpaper-daemon-race` | The first wallpaper apply raced `awww-daemon` start-up; a failed colour extraction was never retried. |
| `fix/idle-cpu` | The cava visualisation tick slows down during silence (tokio wakeups in silence from 81 to about 22 per second). |
| `fix/notification-store-xdg-data-home` | The notification database ignored `XDG_DATA_HOME`. |
| `pick/47-network-active-wired` | [wayle-services#47](https://github.com/wayle-rs/wayle-services/pull/47) bind to the active wired device. |
| `pick/44-media-art-fallback` | [wayle-services#44](https://github.com/wayle-rs/wayle-services/pull/44) thumbnail fallback from the page URL. |

## Building the combined result

The wayle repository builds against the service crates published on crates.io (patched here to Walt's git); to use the service branches, check them out next to this repository and patch every `wayle-*` service crate to the local path:

```bash
git clone https://github.com/beefsizzle/wayle-services && cd wayle-services
git submodule update --init                    # vendored cava sources
git checkout -b my-build master && git merge fix/audio-reconnect fix/systray-races fix/wallpaper-daemon-race fix/idle-cpu
cd ..
git clone https://github.com/beefsizzle/wayle && cd wayle
git checkout -b my-build master && git merge fix/radius-none-unit feat/ext-background-effect perf/shell-lean-runtime   # and any others
```

Then replace the `[patch.crates-io]` block in `Cargo.toml` with `path = "../wayle-services/<crate>"` entries (one per `wayle-*` service crate) and build:

```bash
cargo build --release -p wayle -p wayle-settings        # or: --profile release-small
GSK_RENDERER=vulkan target/release/wayle shell           # optional: see the memory table
```

If cargo warns `patch ... was not used`, run `cargo update -p <crate>` for the crates named; the lock file may still pin the registry version. Keep that patch block out of any pull request.

## How it was tested

* `cargo test --workspace` in both repositories (about 840 tests in this one, about 730 in the services repo), plus `#[ignore]`d suites that need an environment: the audio crate's private-PipeWire tests, the systray crate's private-D-Bus tests, and the CSS matrix (`gtk4-broadwayd :5 &` then `GDK_BACKEND=broadway BROADWAY_DISPLAY=:5 cargo test -p wayle-styling --test css_matrix -- --ignored`).
* Behaviour was checked in a nested niri window with a private D-Bus bus and scratch config, so a running desktop shell was never touched, with screenshots for the visual changes (settings window, blur) and `/proc` sampling for the memory figures.
* **Not covered:** other compositors than niri, GPUs other than the AMD one used, real multi-monitor hot-plug, and fractional scaling beyond a nested test. `flake.nix`'s `cargoHash` has not been refreshed for the changed `Cargo.lock`, so Nix builds of these branches need that updated first.

---

## Base fork: waltmck/wayle (original README)

*The text below is [@waltmck](https://github.com/waltmck)'s README for the fork this repository is based on, kept as written.*

This is a fork of Wayle for testing my experimental changes prior to upstreaming. Currently outstanding PRs included in this branch:
- Fix buggy link-time ordering that randomly breaks builds. [#324](https://github.com/wayle-rs/wayle/pull/324)
- An IWD module for controling WiFi without NetworkManager. [#300](https://github.com/wayle-rs/wayle/pull/300) [#35](https://github.com/wayle-rs/wayle-services/pull/35)
- A rewritten systray module that fixes several race conditions. [#34](https://github.com/wayle-rs/wayle-services/pull/34)
- A fix that makes the `netstat` module work correctly without NetworkManager. [#316](https://github.com/wayle-rs/wayle/pull/316)
- Use absolute paths for `/bin/sh` to fix certain functionality when run from a systemd service. [#316](https://github.com/wayle-rs/wayle/pull/317)
- [WIP] Allow passing a config file location with `-c` or `--config`, enabling a declarative Nix-based module without home-manager. [#318](https://github.com/wayle-rs/wayle/pull/318)
- Rewritten notification service, that is much more efficient with large numbers of notifications and supports multiple backends [#321](https://github.com/wayle-rs/wayle/pull/321) [#38](https://github.com/wayle-rs/wayle-services/pull/36):
  - Direct dbus actions to the notification owner's bus name to work around broken apps (matching GNOME and dunst)
  - Remove non-functional action buttons from orphaned FDO notifications
  - Add support for the `org.gtk.Notifications` backend, which allows notification actions to persist app restarts (only for apps that support `org.gtk.Notifications`, in practice GTK/GApplications).
  - Make batch dismissals ("dismiss all") do atomic database writes and atomic re-renders for drastic performance improvement with many notifications.
  - Minimize widget churn on adding/removing notifications
  - Deduplicate icon images in memory
  - Fix bug that made notification hints silently fail to load from the database on startup.
  - Support for `org.freedesktop.portal.Notification` backend, allowing notifications to work correctly from Flatpaks. Supersedes [#333](https://github.com/wayle-rs/wayle/pull/333)
- Implement systemd `Type=notify` support to avoid use-before-setup race conditions on startup. [#323](https://github.com/wayle-rs/wayle/pull/323)
- Add `general.symbolic-icon-fallback` option to fall back to a symbolic desktop icon if there is no hardcoded symbolic icon (applies to notification and workspace modules). [#325](https://github.com/wayle-rs/wayle/pull/325)
- Pointer cursor on hover over workspace buttons, matching other clickable elements in the shell. [#326](https://github.com/wayle-rs/wayle/pull/326)
- Redesigned and refactor dropdown menus: switched from autohide popovers to a full-screen transparent scrim, similar to Astal/AGS and eww. [#328](https://github.com/wayle-rs/wayle/pull/328)
  - Fixes [#62](https://github.com/wayle-rs/wayle/issues/62) and [#285](https://github.com/wayle-rs/wayle/issues/285)
  - Fully scrollable and arrow key (and `hjkl`) navigable systray dropdows.
  - More responsive systray dropdowns from pre-building on registration rather than rebuilding whenever they are opened.
  - Allow switching between dropdown menus with a single click, rather than having to click twice (one to close the existing menu and one to open the next one).
  - The bar and dropdown will stay on top of full-screen apps when there is a dropdown open
  - CLI dispatchers to open, close, and toggle dropdown menus:
    - `wayle dropdown list` lists available dropdown menus. These are addressed by IDs `dropdown@parent-module`, i.e. `weather@clock` vs `calendar@clock`; duplicates are numbered left to right (i.e. `dashboard@dashboard#1` vs `dashboard@dashboard#2`). The list is live-updated from the configuration (any module with a `dropdown:<name>` action).
    - `wayle dropdown toggle <name>` toggles a given dropdown menu by name
    - `wayle dropdown open <name>` opens a dropdown menu by name (no-ops if already open)
    - `wayle dropdown close` closes all dropdowns (including systray dropdowns)
    - `wayle systray {open, toggle}` functions similarly, opening/closing systray right-click menus by ID.
    - The above open/toggle/close commands all act on the currently active monitor by default (this is determined in a compositor-specific way; they act on all monitors as a fallback). This can be overridden by passing `--monitor=<name>` (to act on a specific monitor) or `--monitor=all` (to act on all monitors).
- Better heuristics to reduce button resize jitter. [#339](https://github.com/wayle-rs/wayle/pull/339)
- Module for Mullvad VPN control/status. [#341](https://github.com/wayle-rs/wayle/pull/341) [#43](https://github.com/wayle-rs/wayle-services/pull/43)
- CLI for controlling OSD [#343](https://github.com/wayle-rs/wayle/pull/343)
- Fixes [#156](https://github.com/wayle-rs/wayle/issues/156) and other miscellaneous bugs in Media module 
- Full rewrite of wayle-bluetooth to fix many race conditions and keep the UI in state with bluez [#373](https://github.com/wayle-rs/wayle/pull/373) [#48](https://github.com/wayle-rs/wayle-services/pull/48)
- Click guards on menus to prevent accidental clicks when buttons move under your cursor (implemented in bluetooth and IWD modules)

Roadmap:
- Implement modules:
  - Syncthing (sync status, etc.)
  - systemd-networkd (exposes dbus API to get/set status of managed interfaces). Need to think about how this can/should interface with Network module
  - ZFS (pool status, dataset usage, health)
  - mpd (play, pause, select song/album from music library)

Please feel free to test these changes and report any issues so that I can fix them before upstreaming.

## Installation

This package can be built normally with `cargo build`. It is also packaged as flake for fellow Nix enjoyers.

### Flake outputs

| Output | Description |
|--------|-------------|
| `packages.<system>.wayle` / `.default` | The `wayle` package (stolen from `nixpkgs`, plus this fork's source and the `wayle.portal` install) |
| `devShells.<system>.default` | A dev shell (alternatively use `devenv`) |
| `overlays.default` | Overlay that overrides `pkgs.wayle` |
| `nixosModules.default` | The `services.wayle` NixOS module (declarative config + `Type=notify` user service + notification portal) |
| `formatter.<system>` | `nixfmt-rfc-style`, for `nix fmt` |

Supported systems: `x86_64-linux`, `aarch64-linux`.

### Building/running with Nix

```bash
nix run .#wayle -- shell            # start the shell
nix run .#wayle -- systray list     # any wayle subcommand
# or, after `nix build`:
./result/bin/wayle shell
```

### Development shell

```bash
nix develop
# inside the shell, the usual cargo workflow works:
cargo build -p wayle
cargo run -p wayle -- shell
```

The shell inherits every build/native input from the package (pkg-config, GTK/Wayland
libraries, `cmake`, the bindgen/clang setup for libcava) and adds `cargo`, `rustc`,
`clippy`, `rustfmt`, and `rust-analyzer`.

### Installation with NixOS

#### NixOS module (`services.wayle`, recommended)

The flake exports a NixOS module (`nixosModules.default`) that installs Wayle, generates
its config declaratively, runs it as a `Type=notify` user service, and registers its
notification portal so notifications work from Flatpaks — no home-manager required.

```nix
{
  inputs.waltmck-wayle.url = "github:waltmck/wayle";   # or "git+file:///path/to/wayle"

  outputs = { nixpkgs, waltmck-wayle, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        waltmck-wayle.nixosModules.default
        ({ pkgs, ... }: {
          services.wayle = {
            enable = true;

            # Fetch the prebuilt wayle binary from the waltmck Cachix cache
            # instead of compiling it from source (see "Binary cache" below).
            useCache = true;

            # Your config.toml as Nix; generated to /etc/wayle/config.toml and passed via --config.
            settings = {
              # general.symbolic-icon-fallback = true;
              # bar.modules.left = [ ... ];
            };

            # Tools Wayle shells out to at runtime (placed on the service's PATH).
            deps = with pkgs; [ bash coreutils ];

            # Icon themes exposed to the shell via XDG_DATA_DIRS.
            iconThemes = with pkgs; [ adwaita-icon-theme papirus-icon-theme ];
          };

          # Required for the notification portal (and portals in general) to run.
          xdg.portal.enable = true;
        })
      ];
    };
  };
}
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | `false` | Install Wayle and run it as a user service. |
| `settings` | TOML (attrset) | `{}` | Written to `/etc/wayle/config.toml`; the shell starts with `--config`. |
| `package` | package | this flake's `wayle` | Read-only; the fork build (bundles `wayle.portal`). |
| `useCache` | bool | `false` | Add the `waltmck` Cachix cache as a substituter so `package` is fetched prebuilt instead of compiled — see the note below. |
| `deps` | list of package | `[]` | Added to the service's `PATH`. |
| `iconThemes` | list of package | `[]` | Their `share/` directories are appended to `XDG_DATA_DIRS`. |

When enabled the module also sets `xdg.portal.extraPortals` and selects Wayle as the
`org.freedesktop.impl.portal.Notification` backend. That only takes effect with
`xdg.portal.enable = true`, which you must set yourself (shown above).

##### Binary cache

Setting `useCache = true` adds the [`waltmck` Cachix cache](https://waltmck.cachix.org)
as a Nix substituter (and trusts its public key), so the wayle package is fetched
prebuilt instead of compiled from source. It is *appended* to your substituters, so
`cache.nixos.org` is preserved.

This actually hits the cache because `services.wayle.package` is the flake's own
pre-built output — built with Wayle's pinned nixpkgs, which is the exact derivation CI
pushes to the cache. A wayle rebuilt against a *different* nixpkgs (e.g. via the overlay)
gets a different store path and misses.

Note the bootstrap: the substituter only becomes active *after* the rebuild that turns
`useCache` on, so the very first `nixos-rebuild switch` still builds from source. Do that
first switch once with

```bash
nixos-rebuild switch --flake … \
  --option extra-substituters      https://waltmck.cachix.org \
  --option extra-trusted-public-keys waltmck.cachix.org-1:jRubmCo4HVNZsnL8+GgsMLaOyi+pMLSuC5/QjYDjWW0=
```

or simply switch a second time afterward, and subsequent rebuilds fetch from the cache.

#### NixOS (package only)

To install just the binary — bringing your own config and service — use the overlay or the
package directly:

```nix
{
  inputs.waltmck-wayle.url = "github:waltmck/wayle";   # or a fork / "git+file:///path/to/wayle"

  outputs = { nixpkgs, waltmck-wayle, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        {
          nixpkgs.overlays = [ waltmck-wayle.overlays.default ];
          environment.systemPackages = [ pkgs.wayle ];
        }
        # ... OR ...
        {
          environment.systemPackages = [ inputs.waltmck-wayle.packages.${pkgs.system}.default ];
        }
      ];
    };
  };
}
```

#### home-manager (flake)

You must also add `wayle` to your inputs, and then

```nix
{ inputs, pkgs, ... }: let
  wayle = inputs.waltmck-wayle.packages.${pkgs.system}.default;
in {
  home.packages = [ wayle ];

  # To make HM's Wayle module use waltmck/wayle
  services.wayle.package = wayle;
}
```
