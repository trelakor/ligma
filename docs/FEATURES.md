# Features

How Ligma behaves, in detail. For the short version, see the [README](../README.md).

## Tabs

- Tabs like the Figma desktop app, in place of the native title bar (on Windows the tab bar draws its own minimize, maximize and close buttons). Home (`https://www.figma.com/files`) is pinned on the left. Files opened from Home get their own tab, or focus the tab already showing them.
- Right-click a tab to pin or unpin it, copy its link, open it in the browser, reload it, or close it, the other tabs or all tabs. Pinned tabs sit after Home and show only their icon. Home's menu has Copy Link, Reload and Close Other Tabs.
- A file whose name starts with an emoji shows that emoji as its tab icon.
- With `"restoreTabs": true` (Settings > Reopen tabs from last time), Ligma reopens your tabs, pins and the tab you had open. They're kept in `session.json` next to `settings.json`. Only the tab you had open loads; the others load when selected.
- Shortcuts: `Ctrl+T` (`Cmd+T` on macOS) new tab, `Ctrl+W` (`Cmd+W`) close tab, `Ctrl+Tab` / `Ctrl+Shift+Tab` switch tabs. While the settings window has focus, `Ctrl+W` (`Cmd+W`) closes it instead. Middle-click also closes a tab.
- Background tabs give memory back (see [Memory](MEMORY.md)).

## Browsing

- Links inside `figma.com` stay in the app. All other web links open in the default browser.
- Window size and position are saved between launches.
- Drag-and-drop goes to Figma, so you can drop images and files onto the canvas.
- On macOS, the app sends a Safari user agent so Figma treats the webview as a supported browser.

## Sign in with Google

Continue with Google signs in the way the Figma desktop app does, because Google doesn't allow sign-in inside embedded webviews: you approve Ligma on Figma's own page, signed in to Figma in a real browser, and Figma hands the sign-in back through a `figma://app_auth/redeem` link to the page you started from, which completes it. Figma only sends that link to `figma://`, whichever app asked (see `src-tauri/src/auth.rs`). Add account in Figma's account menu opens Figma's sign-in page in a new tab, and Continue with Google there works the same way, then switches that tab to the added account.

- **macOS:** the approval page opens in Apple's sign-in sheet, which shares Safari's cookies and catches the `figma://` link itself, so it works alongside the Figma desktop app (`web_auth.rs`).
- **Windows:** the page opens in your browser. Without the Figma desktop app, Ligma registers itself for `figma://` and also opens Figma's "Open in desktop app" links in a tab. With it, Ligma borrows `figma://` only while a sign-in waits (up to 15 minutes) and then puts Figma's entry back, also at the next launch if Ligma quit in between (`figma_scheme.rs`). A second Ligma started by a link passes it on and exits.
- A sign-in link is only accepted while a sign-in Ligma started is waiting, so a link from anywhere else can't sign you into another account.

## Local fonts

Figma loads installed fonts through its font helper, `FigmaAgent`. If the helper isn't running, Ligma starts it (from `~/Library/Application Support/Figma/FigmaAgent.app` on macOS, `%LOCALAPPDATA%/FigmaAgent/figma_agent.exe` on Windows) and stops it on quit. The helper is installed by the Figma desktop app or [Figma's font installer](https://www.figma.com/downloads/). On Windows, Figma also waits for the browser's Local Network Access permission, which WebView2 reports as "prompt" but never asks for, so Ligma reports it as granted to Figma tabs.

## Discord

- **Rich Presence** shows "Ligma" with the open file ("Editing …"), where you are in it and the selected tool. Under the file's name it shows the page's name in Figma Design and FigJam, the slide in Figma Slides and while presenting ("Slide 3 of 12", or "Frame 3 of 40" in a prototype), the chat's name in Figma Make, which asset you're on in Figma Buzz ("Asset 2 of 5") and the webpage's name in Figma Sites. The tool's icon (an art asset of the Discord application, see `scripts/discord-assets.mjs`) is the small image, named when hovered: blue while Ligma has focus, grey when you switch away.
- Settings can hide the file name (it then says "Editing a file", and where you are is hidden with it), the tool, or where you are for each product, or turn it off. In `settings.json` these are `"discordShowFile"`, `"discordShowTool"`, `"discordShowPage"`, `"discordShowFigjamPage"`, `"discordShowSlide"`, `"discordShowChat"`, `"discordShowAsset"`, `"discordShowWebpage"` and `"discordRichPresence"`.
- **Invites to files** (off by default, `"discordInvites"`) makes the open file a Discord party, sized by the people Figma shows in it. Friends can be invited from the + menu in a Discord chat, or ask to join from your profile, which Ligma asks you about. Joining opens the file in their Ligma, starting it if needed, and Figma shows it to them if they have access. It needs Rich Presence on for both of you.

## Settings

Settings (macOS: Ligma > Settings…, `Cmd+,`; Windows: the gear next to minimize) holds the options from `settings.json`, one page per category: Appearance (color scheme, themes and interface scale), Tabs & Memory, Discord, Plugins and About. Changes are saved and apply right away.

`settings.json` is created on first launch in `%APPDATA%\dev.ligma.client` (Windows) or `~/Library/Application Support/dev.ligma.client` (macOS). After editing it by hand, restart Ligma.

## Appearance

- **Color scheme** (`"colorScheme"`: `"system"`, `"light"` or `"dark"`) sets light or dark for Figma and Ligma together and takes the place of Figma's own Theme setting, which Ligma keeps at System.
- **Themes** can be picked for each mode (`"lightTheme"`, `"darkTheme"`): Figma's own colors (`"figma"`), or Grove, Lagoon, Ocean, Midnight, Iris, Rose, Ember or Amber, which tint Figma's greys and give every editor's brand color (buttons, selection, menus; blue in Design, purple in FigJam and so on) the theme's color. The colors that stand for a product, like the Design and FigJam icons and buttons on Home, stay. In light mode, Figma's menus and tooltips are light too, in every theme (Figma draws them dark).
- Themes recolor Figma's panels, toolbars, menus, tooltips and buttons, and what Figma draws over the canvas (selection, rulers, labels), and with them the tab bar, the tab menu, prompts and the settings window, but not your designs: new shapes and frames get Figma's usual fills (see `dist/themes.js` and `src-tauri/src/appearance.rs`).
- **Interface scale** (`"interfaceScale"` in percent, 50 to 200) zooms Figma tabs, the tab bar, the tab menu, prompts and the settings window together, like a browser's zoom; − and + step through the usual zoom levels. The tab bar grows with it.
- The tab menu, prompts (update ready, join requests, errors) and the settings window are drawn by Ligma in Figma's style and follow the editor's light or dark theme. On Windows 11 they have no accent-colored border, just the system shadow.
- **Layer previews** (`"layerPreviews"`, off by default) show a small picture of each layer in the layers panel in every mode (Design, Motion and the rest), as Figma does only in Draw mode, with Draw mode's taller rows. Each mode keeps its own tools and right sidebar. Figma renders every preview, so large files may feel slower. Turning it off applies right away; turning it on applies to files that load afterwards (see `src-tauri/src/layers.rs`).

## Plugins in the left rail

- Your plugins sit in the editor's left rail, under Figma's own buttons: development plugins, then every plugin and widget saved to your Figma account, each with its icon. Click a plugin to run it, or to pick from its commands if it has some; click a widget to put it on the canvas. Icons grow on hover.
- Right-click one there for its menu, like its … menu in Actions > Plugins & widgets: View details and Remove for saved plugins and widgets, Remove for development plugins.
- Settings > Plugins > Left rail shows or hides plugins (`"railPlugins"`) and widgets (`"railWidgets"`). With both off, the rail and its separator are gone.
- Hold Ctrl and drag a plugin to move it. The order is kept for every file; newly saved plugins come first.

## Development plugins

Like the Figma desktop app's Plugins > Development > Import plugin from manifest…, which figma.com doesn't have.

- Settings > Plugins imports a plugin's `manifest.json` (built: `main` has to be JavaScript), switches it on or off, shows its folder or removes it. The list is `plugins.json` next to `settings.json`.
- Plugins that are on show in Figma's Actions panel under Plugins & widgets, as in the desktop app: marked Development, with their menu commands, Remove in their … menu, and a Development filter that lists only them with Import from manifest…. They show in the files whose editor they're made for (Design, FigJam, Slides or Buzz).
- Their files are read on every run, so a rebuilt plugin runs its new code.
- Ligma runs the code against the plugin API Figma's web editor offers on the page (`window.figma`, as in the developer console), so it needs a file you can edit. `figma.showUI`, messages with the UI, `figma.command`, the `run` and `close` events and `closePlugin` work as in the app. `figma.clientStorage` is kept per plugin in figma.com's IndexedDB.
- **Unlike the desktop app, plugins aren't sandboxed:** they run with the page's access, so import only ones you trust.
- Widgets, plugin parameters (quick actions) and the manifest's `build` command aren't supported (see `src-tauri/src/plugins.rs`).

## Updates

On launch and every 6 hours Ligma checks the latest release, downloads a newer version in the background and offers to restart. Choose Later and it installs when you quit. The app is replaced in place, so you stay signed in. If an update can't be installed, Ligma says why and offers to download it or report it.

## Reporting problems

Use Report… in Settings (or Help > Report a Problem… on macOS). It opens a GitHub issue filled in with Ligma's version, the system and the end of `ligma.log`, for you to check and submit. The log is in `%LOCALAPPDATA%\dev.ligma.client\logs` on Windows and `~/Library/Logs/dev.ligma.client` on macOS.

## Security

Figma pages have no access to Tauri IPC. Only the local tab bar has permissions (`src-tauri/capabilities/tab-bar.json`).
