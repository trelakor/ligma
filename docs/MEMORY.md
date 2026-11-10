# Memory

Ligma itself uses a few MB. Nearly all of its memory is Figma: every loaded file keeps its whole editor (WASM engine, canvas, document) in a WebView2/WKWebView renderer, hundreds of MB to several GB each. So Ligma winds down tabs you aren't looking at.

## How hidden tabs wind down

- **On hide (Windows):** WebView2 is asked to trim the tab's memory (`MemoryUsageTargetLevel` low). The page keeps running.
- **After 5 minutes hidden:** the page is frozen and resumes when you switch back. Figma reconnects to its servers. On Windows that's `TrySuspend`, which also trims memory. On macOS it's WebKit's private `_suspendPage:`, which saves CPU but not memory.
- **Hidden tab limit (optional, all platforms, `maxHiddenTabsLoaded`):** past the limit, switching away from a tab puts the oldest hidden one to sleep right away. Home, which reloads quickly, always goes first.
- **After 30 minutes hidden (all platforms):** the tab goes to sleep. Its webview is closed, the tab is dimmed, and it reloads when selected. With `reloadSnapshots` on macOS, a file tab shows its last image, with a soft shimmer passing over it, until Figma has drawn the file again (up to 20 seconds).
- **While macOS is short of memory:** tabs go to sleep sooner, after 10 minutes hidden when macOS warns of memory pressure and at once when pressure is critical. WKWebView has no way to trim a page, so sleeping is the only step that frees a hidden file's memory on macOS. WebKit already slows timers in hidden tabs on its own.

## Game mode

While a game is in front, every tab goes to sleep, the one you're looking at too, and Ligma gives back the memory its own process isn't using, so the game gets it. Game mode lasts while that game runs and Ligma isn't in front, so switching to Discord mid-game wakes nothing. Once you're back in Ligma, the tabs on screen reload; the others reload when selected.

The app in front counts as a game when:

- its executable is in a game library: Steam (`steamapps/common`), Epic Games, GOG, Xbox (`XboxGames`), Riot Games, EA Games or Ubisoft Connect, their launchers aside
- on Windows, it runs in exclusive full screen, or Windows knows it as a game (Game Bar's list in `HKCU\System\GameConfigStore\Children`, which Windows' own Game Mode goes by)
- on macOS, its bundle's category is a game one (`LSApplicationCategoryType` `public.app-category.games` or `…-games`), which macOS' own Game Mode goes by

Turn it off with `"gameMode": false` (Settings > Tabs & Memory > Game mode).

Freezing and sleeping are skipped while you're offline or while Figma's `beforeunload` handler says leaving would lose edits.

## Settings

Change these in Settings > Tabs & Memory (applies right away) or in `settings.json` (restart after editing; see [Features](FEATURES.md#settings) for where it lives):

```json
{ "sleepTabsAfterMinutes": 30, "maxHiddenTabsLoaded": null, "reloadSnapshots": false, "gameMode": true }
```

- `"sleepTabsAfterMinutes": 0` keeps every tab loaded, whatever the limit or memory pressure.
- `"maxHiddenTabsLoaded": null` (the default) sets no limit; `1` keeps only the last tab you left loaded and `0` only the one you're looking at, trading reloads for memory.
- `"reloadSnapshots": true` turns on the snapshots.
- Tab previews (`"tabPreviews"`, on by default) keep each hidden tab's last image in memory as a JPEG, a few hundred KB per tab. `"tabPreviews": false` drops them.

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
