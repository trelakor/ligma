# Changelog

Every release needs a section here, headed `## <version> - <date>`, before it's tagged. The release workflow puts that section in the GitHub release notes and fails without it.

## 0.21.0 - 2026-10-25

- Saved widgets in the editor's left rail, next to your plugins. Click one to put it on the canvas; right-click for View details and Remove.
- Every plugin saved to your Figma account now shows in the rail, as in Actions > Plugins & widgets. Some newer saves were missing before.
- Settings > Plugins > Left rail shows or hides plugins and widgets there. With both off, the rail's separator goes too.

## 0.20.0 - 2026-10-25

- Tab shortcuts on Windows, as on macOS: Ctrl+T opens a new tab, Ctrl+W closes the tab, and Ctrl+Tab and Ctrl+Shift+Tab switch tabs. They work in Figma, the tab bar and Settings, where Ctrl+W closes the window instead.
- A cleaner README, with the details moved to docs/FEATURES.md, docs/MEMORY.md and docs/DEVELOPMENT.md.

## 0.19.0 - 2026-10-25

- Your plugins in the editor's left rail, under Figma's own buttons: development plugins, then every plugin saved to your Figma account, each with its icon. Click one to run it, or to pick from its commands if it has some.
- Right-click a plugin there for its menu, like its … menu in Actions > Plugins & widgets: View details and Remove for saved plugins, Remove for development plugins.
- Hold Ctrl and drag a plugin to move it. The order is kept for every file; newly saved plugins come first.

## 0.18.0 - 2026-10-25

- Number fields in Settings scrub like Figma's: drag one right to raise its value or left to lower it, with Shift in steps of 10. Click without dragging to type in it as before.

## 0.17.1 - 2026-10-25

- Fixed a pixel-wide white or black line beside the layers or properties panel at some interface scales.

## 0.17.0 - 2026-10-24

- Color scheme and themes in Settings > Appearance. Pick System, Light or Dark for Figma and Ligma together (it takes the place of Figma's own Theme setting), and a color theme for each mode: Figma's own colors, or Grove, Lagoon, Ocean, Midnight, Iris, Rose, Ember or Amber.
- A theme tints Figma's panels, toolbars, menus and buttons and gives every editor's brand color its accent, in Design, FigJam, Slides, Buzz, Sites, Make and Dev Mode. The tab bar, tab menu, prompts and settings window follow right away. Product colors, like the Design and FigJam icons on Home, stay.
- On the canvas, selection, rulers and labels take the theme too. Your designs don't: new shapes and frames get Figma's usual fills.
- In light mode, Figma's menus and tooltips are light, in every theme.

## 0.16.0 - 2026-10-24

- Development plugins: import a Figma plugin from your computer by its manifest.json, like the Figma app's Import plugin from manifest…. Settings has a Plugins page to import them, switch them on or off and remove them.
- Plugins that are on show in Figma's Actions panel under Plugins & widgets, marked Development, with their commands, Remove in their … menu and a Development filter that lists only them with Import from manifest…. Arrow keys and Enter work on them like on Figma's own rows. They aren't sandboxed like in the Figma app, so only import plugins you trust.
- Fixed community plugins on Windows failing to start with "An error occurred while loading the plugin environment".

## 0.15.2 - 2026-10-24

- If Windows won't start an update's installer (security software such as Smart App Control can block it), Ligma no longer stays open in the background with no window. It restarts and says the update failed, with the download page to install it by hand.

## 0.15.1 - 2026-10-24

- Fixed the Figma Design switch in Settings > Discord turning itself off whenever another setting changed. If it's off, switch it back on once.
- Where you are in the file now only shows on Discord with the file's name. Its switches sit under Show file name in Settings and turn off with it.

## 0.15.0 - 2026-10-24

- Settings has a page for each category: Appearance, Tabs & Memory, Discord and About.
- New interface scale setting (Settings > Appearance) zooms Figma, the tab bar, menus, prompts and Settings together. − and + step through the usual zoom levels.
- Prompts, such as join requests, update ready and errors, are drawn in Figma's style instead of as Windows message boxes, and open centered over the window.
- The tab menu is light or dark with the editor.
- On Windows 11, Ligma's menus, prompts and Settings have no accent-colored border, just a soft shadow.

## 0.14.0 - 2026-10-24

- Your Discord status shows where you are in the file under its name: the page in Figma Design and FigJam, the slide in Figma Slides or while presenting ("Slide 3 of 12", or the frame of a prototype), the chat in Figma Make, the asset in Figma Buzz ("Asset 2 of 5") and the webpage in Figma Sites. Each Figma product has its own switch in Settings, all on by default.
- The selected tool shows on Discord only as its icon, named when you hover it, instead of also taking up a line of text.
- Figma Buzz tabs show Buzz's pink party popper, Figma Make tabs show Make's silvery pinwheel, and Figma Sites tabs are Sites' deep purple.

## 0.13.1 - 2026-10-24

- Hovering Ligma's logo on your Discord status shows the version of Ligma you're using.

## 0.13.0 - 2026-10-24

- Report a problem from Settings (or Help on macOS). It opens a GitHub issue filled in with Ligma's version, your system and the end of Ligma's log, for you to check and submit. Nothing is sent otherwise.
- Ligma keeps a log, `ligma.log`, of updates and errors. It holds no Figma file names or links.
- Error dialogs offer to report the problem, and if Ligma ran into a problem last time, it offers to report it on the next launch.
- Settings has an About section with Ligma's version and Check for Updates, now on Windows too.

## 0.12.1 - 2026-10-24

- If an update can't be installed, Ligma says why and offers to download it, instead of restarting as the old version. A failed update is offered again from Check for Updates…, and one that fails while quitting is reported on the next launch.
- Windows releases ship only the `-setup.exe` installer. The `.msi` is gone.

## 0.12.0 - 2026-10-24

- New Discord setting, Invites to files (off by default): Discord friends can join the file you have open. Invite them from the + menu in a Discord chat, or accept when they ask to join from your profile. It opens in their Ligma, starting it if needed, and Figma shows it to them if they have access. Your status shows how many people are in the file.
- Ligma's logo on Discord sits on a white rounded tile, like the app icon.

## 0.11.7 - 2026-10-24

- FigJam, Figma Make, Figma Buzz and Figma Sites tabs no longer end in the product's name, in the tab bar or on Discord.
- Discord Rich Presence reads "Designing in", "Jamming in" and so on before the file's name. Presenting still reads "Presenting".

## 0.11.6 - 2026-10-24

- Redesigned Settings in Figma's style, with its own title bar, Figma's switches and number fields, and Figma's light or dark theme. Esc closes it.
- Right-clicking a tab opens a menu in Figma's style that follows Figma's theme, instead of the system menu. Arrow keys and Enter work in it.
- Discord Rich Presence says what you're doing in each Figma product: Designing, Jamming in FigJam, Decking in Slides, Generating in Make, Buzzing in Buzz, and Page-building in Sites.

## 0.11.5 - 2026-10-23

- Presenting a file that's already being presented goes back to its presentation tab instead of opening another.
- Prototype tabs no longer show a ▶ before the file name, since their icon is already a play button, and Figma Slides tabs no longer end in "Figma Slides".
- Discord Rich Presence shows "Presenting" and the file's name while you present from Figma Design or Figma Slides.

## 0.11.4 - 2026-10-23

- Present in Figma Design opens the presentation in a new tab, like in a browser, instead of in place of the file.
- Present in Figma Slides works. It did nothing before.

## 0.11.3 - 2026-10-23

- The selected tool's icon in Discord Rich Presence is sharper. Ligma now uses icons uploaded to its Discord application instead of small images hosted on GitHub.
- Ligma is now MIT licensed. The installers include the license and the licenses of the open source software Ligma uses.
- New Terms of Service and Privacy Policy. Ligma collects nothing; the Privacy Policy explains what Discord Rich Presence shares.

## 0.11.2 - 2026-10-23

- Add account in Figma's account menu now works on Windows. It opens Figma's sign-in page in a new tab, where Continue with Google signs in the same way as the first time and then shows the added account.
- Links Figma opens in a new window, like Add account, no longer do nothing on Windows.
- Signing in with Google finishes in the page you started from, like in the Figma desktop app, and says so if Figma doesn't accept it.

## 0.11.1 - 2026-10-23

- Continue with Google now finishes signing in. In 0.11.0 the approval in your browser went to the Figma desktop app, or nowhere, instead of back to Ligma.
- On macOS, the approval page opens in a sign-in sheet that uses your Safari sign-in, and works alongside the Figma desktop app.
- On Windows, the approval page opens in your browser. Without the Figma desktop app installed, Ligma also opens Figma's "Open in desktop app" links in a tab. With it, Ligma takes over those links only while you sign in, then hands them back.

## 0.11.0 - 2026-10-23

- Continue with Google now works. Ligma opens Figma's approval page in your browser, where you sign in with Google and approve, then brings the sign-in back to the tab you started from. Google doesn't allow sign-in inside embedded webviews, so this is how the Figma desktop app does it too.
- On Windows, opening Ligma again brings the open window forward instead of starting a second copy.

## 0.10.0 - 2026-10-23

- New Discord Rich Presence settings: Show file name (when off, Discord says "Editing a file") and Show selected tool (when off, the tool's name and icon are hidden).
- On Windows, the Home tab now takes its width from Figma's left sidebar, so it lines up at any display scaling.
- The Settings window is tall enough for all its options again. It no longer reopens at a size saved by an older version.

## 0.9.0 - 2026-10-23

- New Settings button on Windows, next to the minimize button. It opens the same Settings window as Ligma > Settings… on macOS.
- Files whose names start with an emoji now show that emoji as their tab icon on Windows too.
- On Windows, the Home tab lines up with Figma's left sidebar, and its divider continues into the sidebar's border.

## 0.8.0 - 2026-10-23

- Right-click a tab to pin or unpin it, copy its link, open it in your browser, reload it, or close it, the other tabs or all tabs. Home's menu has Copy Link, Reload and Close Other Tabs. Close Other Tabs keeps pinned tabs.
- Pinned tabs sit right after Home and show only their icon.
- Drag tabs to reorder them. Pinned tabs move among themselves, the others among themselves.
- New setting, Reopen tabs from last time: brings back your tabs, pins and the tab you had open. Only that tab loads; the others load when you select them.
- Ligma > Check for Updates… checks right away.
- A file whose name starts with an emoji shows that emoji as its tab icon.
- The Figma Design tab icon's pen nib is angled like Figma's.
- "Open in new tab" on Home now opens the file in a new Ligma tab.
- Fixed a crash on macOS 27 when closing a tab or when a tab went to sleep.

## 0.7.0 - 2026-10-23

- Exporting several images at once saves them into a folder you choose, like the Figma desktop app, instead of as a zip. Files already in the folder are kept: new ones get a " (2)" suffix.

## 0.6.1 - 2026-10-23

- Discord Rich Presence now shows the blue tool icon while Ligma has focus. It showed the grey one whenever a file was open.

## 0.6.0 - 2026-10-23

- New Settings window on macOS: Ligma > Settings… (Cmd+,). Changes to tab sleep, the hidden-tab limit, reload snapshots and Discord Rich Presence apply at once, without a restart.
- Discord Rich Presence shows the selected tool's icon: blue while Ligma has focus, grey otherwise. Text on path, Annotation and Measurement are now detected too.
- Figma downloads ask where to save, like a browser, instead of being dropped.
- Release notes now come from this changelog.

## 0.5.0 - 2026-10-23

- Ligma now updates itself. On launch and every 6 hours it checks for a new release, downloads it in the background and offers to restart. Choose Later and it installs when you quit. Your sign-in and settings stay.
- Updates are signed and verified before they're installed.

## 0.4.0 - 2026-10-23

- Less memory on macOS: hidden tabs sleep after 10 minutes while macOS warns of memory pressure, and at once when pressure is critical.
- Tabs hidden for 5 minutes are now frozen on macOS too, saving CPU.
- New `maxHiddenTabsLoaded` setting limits how many hidden tabs stay loaded. Home goes to sleep first.
- New `reloadSnapshots` setting (macOS): a file tab waking from sleep shows its last image until Figma has drawn the file.
- The traffic lights are centered on the tab bar, which is now 38px tall.
- Tab hover now clears when the mouse leaves the tab bar.
- Added `scripts/measure-memory.sh` to measure memory use on macOS.

## 0.3.0 - 2026-10-23

- Discord Rich Presence shows Ligma, the open file and the selected tool. Turn it off with `"discordRichPresence": false` in `settings.json`.
- Figma Make and Sites previews (`*.figma.site`) stay in the page on macOS instead of opening in the browser.

## 0.2.1 - 2026-10-22

- New Ligma icon on Windows.

## 0.2.0 - 2026-10-22

- Background tabs give memory back. On Windows a hidden tab's memory is trimmed at once and the tab is frozen after 5 minutes. On all platforms a tab hidden for 30 minutes goes to sleep and reloads when selected (`sleepTabsAfterMinutes` in `settings.json`).
- Freezing and sleeping are skipped while offline or while Figma has unsaved edits.
- Added `scripts/measure-memory.ps1` to measure memory use on Windows.

## 0.1.1 - 2026-10-22

- Local fonts work on Windows. Ligma also starts Figma's font helper on Windows when it isn't running.

## 0.1.0 - 2026-10-22

- First release: Figma in the system webview, with a Figma-style tab bar, theme-matched colors and local fonts through Figma's font helper. Builds for macOS (universal) and Windows.
