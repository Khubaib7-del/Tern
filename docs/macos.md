# Tern on macOS

> Status: built in PR #43 (roadmap phase 12). CI builds and tests Tern on macOS (Apple silicon, arm64) on every pull request and uploads `tern-macos-arm64`, an unsigned app bundle. Tern is not released: there is no signed or notarized build and no installer.

## Get a build

| Way | Steps |
| --- | --- |
| CI artifact | Open a pull request's checks, download `tern-macos-arm64`, and unzip it. It holds `Tern.app`, the `tern` CLI, and a short README |
| From source | Install the Xcode command-line tools (`xcode-select --install`) and Rust, then `cargo build --release -p tern-desktop -p tern-tui`; the programs are in `target/release/`. `cargo run -p tern-desktop` opens the app without a bundle |

### First launch (Gatekeeper)

The bundle is not signed or notarized, so macOS blocks a double-click with "Tern cannot be opened because the developer cannot be verified" (or "is damaged" when the download kept its quarantine flag). To open it:

1. Move `Tern.app` to Applications (or anywhere outside Downloads).
2. Right-click (or Control-click) `Tern.app` and choose **Open**, then **Open** again in the dialog. macOS remembers the choice.
3. On macOS 15 and later, if there is no Open button: try to open it once, then go to **System Settings → Privacy & Security** and choose **Open Anyway**.

From a terminal, `xattr -dr com.apple.quarantine Tern.app` removes the quarantine flag instead. The `tern` CLI may need the same: `xattr -d com.apple.quarantine tern`.

## Build dependencies

| Need | Why |
| --- | --- |
| Xcode command-line tools | The linker and the system frameworks (AppKit, Metal, Security) |
| Rust stable | The workspace's toolchain (`rust-toolchain.toml`) |

Nothing else: the GPU shaders are compiled at run time, so the Metal toolchain that newer Xcode versions download separately is not needed.

## What is OS-specific

| Piece | On macOS |
| --- | --- |
| Window and drawing | GPUI on AppKit, drawn with Metal. The title bar is Tern's own, drawn under a transparent system bar: the traffic lights sit at its left and the bar leaves 80 px for them; in full screen they hide. Buttons and pills in the bar are kept out of its drag region (`occlude`), so clicks reach them; double-clicking empty bar follows the system's double-click setting (zoom or minimize) |
| Menu bar | **Tern** (Hide Tern, Hide Others, Show All, Quit Tern) and **Window** (Minimize, Zoom, Close Window) |
| Keys | Cmd+Q quits, Cmd+W closes the window, Cmd+M minimizes, Cmd+H hides, Option+Cmd+H hides others. Text fields use Cmd for copy, paste, cut, undo, and select all, as the system does |
| Folders | Data and settings in `~/Library/Application Support/Tern`, the crawl cache in `~/Library/Caches/Tern`. `TERN_HOME` puts everything in one folder |
| Keys | The login Keychain, as generic passwords under service `tern` with the entry name as the account (Keychain Access shows them). The first read after an update may ask to allow access, since the build is unsigned |
| Opening files and links | Links open in the default browser; saved reports open as `file://` links in their default app |
| Fonts | IBM Plex Sans and Mono and Instrument Serif are built into the app; Core Text supplies fallbacks for scripts they lack |
| Reduced motion | Read once at start from **System Settings → Accessibility → Display → Reduce motion** (`com.apple.universalaccess reduceMotion`). `TERN_REDUCED_MOTION=1` and `"reduced_motion": true` in `preferences.json` still work |
| Notifications | Tern sends no notifications on any OS yet |

## Run

| Command | Does |
| --- | --- |
| `open Tern.app` | The desktop app |
| `./tern doctor` | Checks the folders, the Keychain, and disk space |
| `./tern provider add groq` | Adds a key to the Keychain (hidden prompt, tested before saving) |

## Known limits

| Limit | Detail |
| --- | --- |
| Apple silicon only in CI | Intel Macs should build from source with the same steps but are not built or tested by CI |
| Unsigned | No Developer ID signature or notarization; Gatekeeper needs the right-click Open the first time |
| No installer | No `.dmg`, Homebrew cask, or Mac App Store build; the zip is a CI test build |
| Keychain prompts | An unsigned app gets a new identity with each build, so the Keychain may ask again after you replace the app |
| Verified on CI only | The app builds, the tests (including a Keychain round trip) and the offline evaluation pass on CI's macOS runner; the window itself has not been tried by hand on a Mac yet |
