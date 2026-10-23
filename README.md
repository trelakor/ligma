<img src="assets/logo.png" width="64" height="64" alt="">

# Ligma

Created in [T3 Code](https://t3.codes).

Lightweight desktop app for [Figma](https://www.figma.com), built with [Tauri 2](https://tauri.app). It opens `figma.com` in the system webview (WKWebView on macOS, WebView2 on Windows). No bundled browser engine, so the app is about 4 MB.

## Download

Get the latest macOS and Windows builds from [Releases](../../releases/latest). Both are unsigned; the release notes explain how to open them.

To publish a release, bump `version` in `src-tauri/tauri.conf.json`, then push a matching tag (`git tag v0.2.0 && git push --tags`). `.github/workflows/release.yml` builds both platforms and attaches the installers.

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
- Figma pages have no access to Tauri IPC. Only the local tab bar has permissions (`src-tauri/capabilities/tab-bar.json`).

## Memory

Ligma itself uses a few MB. Nearly all of its memory is Figma: every loaded file keeps its whole editor (WASM engine, canvas, document) in a WebView2/WKWebView renderer, hundreds of MB to several GB each. So Ligma winds down tabs you aren't looking at:

- On hide (Windows): WebView2 is asked to trim the tab's memory (`MemoryUsageTargetLevel` low). The page keeps running.
- After 5 minutes hidden (Windows): the page is frozen (`TrySuspend`), and resumes when you switch back. Figma reconnects to its servers.
- After 30 minutes hidden (all platforms): the tab goes to sleep. Its webview is closed, the tab is dimmed, and it reloads when selected.

Freezing and sleeping are skipped while you're offline or while Figma's `beforeunload` handler says leaving would lose edits. Set the sleep timeout, or turn sleeping off with `0`, in `settings.json`, created on first launch in `%APPDATA%\dev.ligma.client` (Windows) or `~/Library/Application Support/dev.ligma.client` (macOS):

```json
{ "sleepTabsAfterMinutes": 30 }
```

Restart Ligma after editing it. To see memory use on Windows, summed over Ligma and its WebView2 processes:

```powershell
./scripts/measure-memory.ps1               # Ligma
./scripts/measure-memory.ps1 -Name Figma   # Figma desktop, to compare
```

## Known limits

- "Continue with Google" sign-in needs Google's site, which opens in your browser. Sign in with email/password or SSO instead.
- Windows builds are produced by CI and haven't been tested on a real Windows machine yet. Keyboard tab shortcuts are macOS-only for now.
