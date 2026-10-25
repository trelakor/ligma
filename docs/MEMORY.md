# Memory

Ligma itself uses a few MB. Nearly all of its memory is Figma: every loaded file keeps its whole editor (WASM engine, canvas, document) in a WebView2/WKWebView renderer, hundreds of MB to several GB each. So Ligma winds down tabs you aren't looking at.

## How hidden tabs wind down

- **On hide (Windows):** WebView2 is asked to trim the tab's memory (`MemoryUsageTargetLevel` low). The page keeps running.
- **After 5 minutes hidden:** the page is frozen and resumes when you switch back. Figma reconnects to its servers. On Windows that's `TrySuspend`, which also trims memory. On macOS it's WebKit's private `_suspendPage:`, which saves CPU but not memory.
- **Hidden tab limit (optional, all platforms, `maxHiddenTabsLoaded`):** past the limit, switching away from a tab puts the oldest hidden one to sleep right away. Home, which reloads quickly, always goes first.
- **After 30 minutes hidden (all platforms):** the tab goes to sleep. Its webview is closed, the tab is dimmed, and it reloads when selected. With `reloadSnapshots` on macOS, a file tab shows its last image, with a soft shimmer passing over it, until Figma has drawn the file again (up to 20 seconds).
- **While macOS is short of memory:** tabs go to sleep sooner, after 10 minutes hidden when macOS warns of memory pressure and at once when pressure is critical. WKWebView has no way to trim a page, so sleeping is the only step that frees a hidden file's memory on macOS. WebKit already slows timers in hidden tabs on its own.

Freezing and sleeping are skipped while you're offline or while Figma's `beforeunload` handler says leaving would lose edits.

## Settings

Change these in Settings > Tabs & Memory (applies right away) or in `settings.json` (restart after editing; see [Features](FEATURES.md#settings) for where it lives):

```json
{ "sleepTabsAfterMinutes": 30, "maxHiddenTabsLoaded": null, "reloadSnapshots": false }
```

- `"sleepTabsAfterMinutes": 0` keeps every tab loaded, whatever the limit or memory pressure.
- `"maxHiddenTabsLoaded": null` (the default) sets no limit; `1` keeps only the last tab you left loaded and `0` only the one you're looking at, trading reloads for memory.
- `"reloadSnapshots": true` turns on the snapshots.

## Measuring

On Windows, summed over Ligma and its WebView2 processes:

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
