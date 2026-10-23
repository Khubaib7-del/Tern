# Tern on Linux

> Status: built in PR #43 (roadmap phase 12). CI builds, tests, and evaluates Tern on Ubuntu (x64) on every pull request and uploads `tern-linux-x64`. Tern is not released: there are no packages or installers.

## Get a build

| Way | Steps |
| --- | --- |
| CI artifact | Open a pull request's checks, download `tern-linux-x64`, and unpack it: `tar -xzf tern-linux-x64.tar.gz`. Run `./tern-linux-x64/tern-desktop` for the app or `./tern-linux-x64/tern` for the CLI |
| From source | Install the libraries below, then `cargo build --release -p tern-desktop -p tern-tui`; the programs are in `target/release/` |

The archive holds `tern`, `tern-desktop`, a desktop entry, and the 512 px icon in a `share/` tree. To put Tern in your app menu, copy `share/` into `~/.local/share/` and put `tern-desktop` on your `PATH` (for example in `~/.local/bin`). The entry's `StartupWMClass` matches the window's app id (`io.github.khubaib7_del.Tern`), so the dock shows Tern's icon.

## Build dependencies

| Distribution | Command |
| --- | --- |
| Debian, Ubuntu | `sudo apt install build-essential pkg-config libfontconfig-dev libfreetype-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev libvulkan-dev libssl-dev` |
| Fedora | `sudo dnf install gcc pkgconf-pkg-config fontconfig-devel freetype-devel libxkbcommon-devel libxkbcommon-x11-devel wayland-devel libxcb-devel vulkan-loader-devel openssl-devel` |
| Arch | `sudo pacman -S base-devel fontconfig freetype2 libxkbcommon libxkbcommon-x11 wayland libxcb vulkan-icd-loader openssl` |

At run time the desktop app needs a Vulkan driver (Mesa's `vulkan-radeon`, `vulkan-intel`, or the NVIDIA driver) and either an X11 or a Wayland session. The CLI needs neither.

## What is OS-specific

| Piece | On Linux |
| --- | --- |
| Window and drawing | GPUI on X11 or Wayland (picked from the session), drawn with Vulkan. Under Wayland compositors that grant client-side decorations, Tern draws its own title bar with minimize, maximize, and close; under server-side decorations (X11 without a compositor, some compositors) the window manager's bar is used and Tern's buttons are hidden. Right-click the title bar for the window menu |
| Folders | Data in `$XDG_DATA_HOME/tern` (`~/.local/share/tern`), settings (`preferences.json`, `mcp.json`) in `$XDG_CONFIG_HOME/tern` (`~/.config/tern`), the crawl cache in `$XDG_CACHE_HOME/tern` (`~/.cache/tern`). `TERN_HOME` puts everything in one folder |
| Keys | The Secret Service over D-Bus: GNOME Keyring, KWallet, or KeePassXC with its Secret Service integration. Entries are filed under service `tern` in the login collection |
| Keys without a Secret Service | Set `TERN_KEYRING=keyutils` to keep keys in the kernel's per-user keyring instead. It needs no daemon, but the keys are gone after a reboot. Without either, `tern doctor` says so and key commands fail; Tern never writes a key to a file |
| Opening files and links | Links open with the desktop's default browser through the portal or `xdg-open`; saved reports open as `file://` links |
| Fonts | IBM Plex Sans and Mono and Instrument Serif are built into the app, so text looks the same without them installed. Fontconfig supplies fallbacks for scripts they lack, such as CJK and emoji |
| Reduced motion | Read once at start from GNOME (`org.gnome.desktop.interface enable-animations`, which most GTK desktops follow) or KDE (`AnimationDurationFactor` of 0). `TERN_REDUCED_MOTION=1` and `"reduced_motion": true` in `preferences.json` still work |
| Keys in the app | Ctrl+Q quits. Text fields use the usual Ctrl shortcuts. Ctrl+W is left for tabs |
| Notifications | Tern sends no desktop notifications on any OS yet |

## Run

| Command | Does |
| --- | --- |
| `tern-desktop` | The desktop app |
| `tern doctor` | Checks the folders, the keychain backend, and disk space |
| `TERN_KEYRING=keyutils tern provider add groq` | Adds a key on a machine without a Secret Service |
| `WAYLAND_DISPLAY= tern-desktop` | Forces X11 (XWayland) when a Wayland compositor misbehaves |

## Known limits

| Limit | Detail |
| --- | --- |
| x64 only in CI | arm64 Linux should build from source with the same steps but is not built or tested by CI |
| No packages | No `.deb`, `.rpm`, Flatpak, Snap, or AppImage; the archive is a CI test build |
| Headless keys | A server or container without a desktop session has no Secret Service; use `TERN_KEYRING=keyutils` and expect to re-add keys after a reboot |
| WSL | WSL's Secret Service has no default collection; use `TERN_KEYRING=keyutils` |
| Not yet checked by hand | CI builds the app and runs the tests and the offline evaluation; the window itself has not been tried by hand on a Linux desktop yet |
