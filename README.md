<img src="assets/logo.png" width="64" height="64" alt="">

# Ligma

Created in [T3 Code](https://t3.codes).

Lightweight desktop app for [Figma](https://www.figma.com), built with [Tauri 2](https://tauri.app). It opens `figma.com` in the system webview (WKWebView on macOS, WebView2 on Windows). No bundled browser engine, so the app is about 4 MB.

## Download

Get the latest macOS and Windows builds from [Releases](../../releases/latest). Both are unsigned; the release notes explain how to open them.

Ligma updates itself. On launch and every 6 hours it checks the latest release, downloads a newer version in the background and offers to restart. Choose Later and it installs when you quit. The app is replaced in place, so you stay signed in.

To publish a release, bump `version` in `src-tauri/tauri.conf.json`, add a `## <version> - <date>` section with its changes to [`CHANGELOG.md`](CHANGELOG.md), then push a matching tag (`git tag v0.6.0 && git push --tags`). `.github/workflows/release.yml` stops if the changelog section is missing or the tag doesn't match the version. It writes the release notes from the changelog section with `scripts/release-notes.mjs` (run it locally to preview), builds both platforms, attaches the installers and writes the `latest.json` that installed apps check. It signs the update packages with the `TAURI_SIGNING_PRIVATE_KEY` repository secret (a key from `npx tauri signer generate`, without password), whose public key is `plugins.updater.pubkey` in `tauri.conf.json`. Keep the private key: without it, installed apps can't update.

## Requirements

- [Rust](https://rustup.rs) (stable)
- Node.js (only for the Tauri CLI)

## Run

```sh
npm install
npm run dev     # debug build, opens the app
npm run build   # release bundle in src-tauri/target/release/bundle/
```

Regenerate icons after editing `assets/icon.png` (macOS) or `assets/icon-windows.png` (Windows `icon.ico`):

```sh
npm run icon
```

## Behavior

- Tabs like the Figma desktop app, in place of the native title bar (on Windows the tab bar draws its own minimize, maximize and close buttons). Home (`https://www.figma.com/files`) is pinned on the left. Files opened from Home get their own tab, or focus the tab already showing them.
- Shortcuts (macOS): `Cmd+T` new tab, `Cmd+W` close tab, `Ctrl+Tab` / `Ctrl+Shift+Tab` switch tabs. Middle-click also closes a tab.
- Links inside `figma.com` stay in the app. All other web links open in the default browser.
- Window size and position are saved between launches.
- Drag-and-drop goes to Figma, so you can drop images and files onto the canvas.
- On macOS, the app sends a Safari user agent so Figma treats the webview as a supported browser.
- Local fonts: Figma loads installed fonts through its font helper, `FigmaAgent`. If the helper isn't running, Ligma starts it (from `~/Library/Application Support/Figma/FigmaAgent.app` on macOS, `%LOCALAPPDATA%/FigmaAgent/figma_agent.exe` on Windows) and stops it on quit. The helper is installed by the Figma desktop app or [Figma's font installer](https://www.figma.com/downloads/). On Windows, Figma also waits for the browser's Local Network Access permission, which WebView2 reports as "prompt" but never asks for, so Ligma reports it as granted to Figma tabs.
- Background tabs give memory back (see [Memory](#memory)).
- Discord Rich Presence shows "Ligma" with the open file ("Editing …") and the selected tool. The tool's icon (`assets/tools`) is the small image: blue while Ligma has focus, grey when you switch away. Turn it off in Settings, or with `"discordRichPresence": false` in `settings.json` (see [Memory](#memory) for where it lives).
- Settings (macOS: Ligma > Settings…, `Cmd+,`) holds the options from `settings.json`. Changes are saved and apply right away.
- Figma pages have no access to Tauri IPC. Only the local tab bar has permissions (`src-tauri/capabilities/tab-bar.json`).

## Memory

Ligma itself uses a few MB. Nearly all of its memory is Figma: every loaded file keeps its whole editor (WASM engine, canvas, document) in a WebView2/WKWebView renderer, hundreds of MB to several GB each. So Ligma winds down tabs you aren't looking at:

- On hide (Windows): WebView2 is asked to trim the tab's memory (`MemoryUsageTargetLevel` low). The page keeps running.
- After 5 minutes hidden: the page is frozen and resumes when you switch back. Figma reconnects to its servers. On Windows that's `TrySuspend`, which also trims memory. On macOS it's WebKit's private `_suspendPage:`, which saves CPU but not memory.
- Optionally, only a few hidden tabs stay loaded (all platforms, `maxHiddenTabsLoaded`): past that, switching away from a tab puts the oldest hidden one to sleep right away. Home, which reloads quickly, always goes first.
- After 30 minutes hidden (all platforms): the tab goes to sleep. Its webview is closed, the tab is dimmed, and it reloads when selected. With `reloadSnapshots` on macOS, a file tab shows its last image, with a soft shimmer passing over it, until Figma has drawn the file again (up to 20 seconds).
- While macOS is short of memory: tabs go to sleep sooner, after 10 minutes hidden when macOS warns of memory pressure and at once when pressure is critical. WKWebView has no way to trim a page, so sleeping is the only step that frees a hidden file's memory on macOS. WebKit already slows timers in hidden tabs on its own.

Freezing and sleeping are skipped while you're offline or while Figma's `beforeunload` handler says leaving would lose edits. Set the sleep timeout and how many hidden tabs stay loaded, or turn sleeping off with `0`, in `settings.json`, created on first launch in `%APPDATA%\dev.ligma.client` (Windows) or `~/Library/Application Support/dev.ligma.client` (macOS):

```json
{ "sleepTabsAfterMinutes": 30, "maxHiddenTabsLoaded": null, "reloadSnapshots": false }
```

On macOS, Ligma > Settings… changes these without a restart. After editing the file by hand, restart Ligma. `"maxHiddenTabsLoaded": null` (the default) sets no limit; `1` keeps only the last tab you left loaded and `0` only the one you're looking at, trading reloads for memory. `"reloadSnapshots": true` turns on the snapshots. With `"sleepTabsAfterMinutes": 0` every tab stays loaded, whatever the limit or memory pressure. To see memory use on Windows, summed over Ligma and its WebView2 processes:

```powershell
./scripts/measure-memory.ps1               # Ligma
./scripts/measure-memory.ps1 -Name Figma   # Figma desktop, to compare
```

On macOS, summed over Ligma and the WebKit processes it's responsible for (as Activity Monitor attributes them):

```sh
./scripts/measure-memory.sh                # Ligma
./scripts/measure-memory.sh -n Figma       # Figma desktop, to compare
./scripts/measure-memory.sh -p 1234 -w 5   # one instance by PID, every 5 seconds
```

## Known limits

- "Continue with Google" sign-in needs Google's site, which opens in your browser. Sign in with email/password or SSO instead.
- Windows builds are produced by CI and haven't been tested on a real Windows machine yet. Keyboard tab shortcuts are macOS-only for now.
