# Privacy Policy

Last updated: October 24, 2026

This policy covers Ligma, the desktop app in this repository, and the Ligma application on Discord that it uses for Rich Presence. "We" means the Ligma maintainers.

## The short version

Ligma has no servers and no accounts, and it collects nothing. It sends no analytics, telemetry or crash reports. We never receive your data, unless you choose to report a problem on GitHub.

## What stays on your computer

Ligma keeps a few files in its app data folder (`%APPDATA%\dev.ligma.client` on Windows, `~/Library/Application Support/dev.ligma.client` on macOS):

- `settings.json`: your settings.
- `session.json`: your open and pinned tabs, if Reopen tabs from last time is on.
- The window's size and position.
- `ligma.log`, a log of what Ligma did, such as checking for and installing updates, and errors. It's in Ligma's log folder (`%LOCALAPPDATA%\dev.ligma.client\logs` on Windows, `~/Library/Logs/dev.ligma.client` on macOS) and holds no Figma file names or links.

Figma's own data, such as cookies and your signed-in session, is kept by the system webview (WebView2 on Windows, WKWebView on macOS) on your computer. Uninstalling Ligma and deleting its app data folder removes all of it.

## Discord Rich Presence

If Discord is running, Ligma tells the Discord app on your computer what to show on your profile. This goes over Discord's local connection (IPC) on your machine, not over the internet to us. Ligma sends:

- "Ligma" and the time you started using it.
- The name of the Figma file you have open, shown as "Editing …".
- The Figma tool you have selected, and its icon.
- With Invites to files on (it's off unless you turn it on): how many people Figma shows in the open file, an ID for the file that doesn't reveal it, and a link to the file that Discord keeps to itself until someone joins.

Discord then shows this to other people as part of your activity, under [Discord's Privacy Policy](https://discord.com/privacy). Ligma does not read your Discord account, messages, servers or friends, and doesn't ask for any OAuth2 permissions.

With Invites to files on, everyone you invite to the open file, and everyone whose request to join you accept, gets the link to it. Figma still decides what they can see: without access, they get Figma's page to request it. When a friend asks to join, Discord tells Ligma their Discord name and user ID so Ligma can ask you; Ligma keeps neither. So that joining can start Ligma, Ligma tells Discord where it's installed (in the Windows registry, or in Discord's folder on macOS) while Rich Presence is on.

You choose what's shared in Ligma's Settings: hide the file name (it then says "Editing a file"), hide the tool, turn invites on or off, or turn Rich Presence off. Discord's own Activity Privacy settings can also hide it. Turning it off clears your status right away.

The logo and tool icons are images in this repository on GitHub. Discord loads them from there to display them.

## Problem reports

When something goes wrong, such as an update that can't be installed, Ligma offers to report it. You can also report a problem from Settings, or Help on macOS. Report opens a new issue on Ligma's GitHub page in your browser, filled in with:

- Ligma's version, and your system's name, version and processor type.
- The error, if there was one.
- The last lines of `ligma.log`, with your user name taken out of file paths.

If that's too long for a link, Ligma also copies the full report to your clipboard for you to paste. Nothing is sent until you read it, edit it if you like, and submit it yourself. The issue is public on GitHub, under your GitHub account and [GitHub's Privacy Statement](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement). We use reports only to fix Ligma.

## Other services Ligma connects to

- **Figma.** Ligma opens figma.com. Everything you do in Figma, including signing in, goes to Figma under [Figma's Privacy Policy](https://www.figma.com/legal/privacy/). With Continue with Google, your browser or Apple's sign-in sheet opens Figma's own page, and Figma hands the sign-in back to Ligma on your computer.
- **GitHub.** On launch and every 6 hours, Ligma checks GitHub Releases for an update and downloads it from there. GitHub sees a normal web request, including your IP address, under [GitHub's Privacy Statement](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement).
- **Figma's font helper.** If installed, Ligma starts Figma's `FigmaAgent` on your computer so Figma can use your local fonts.

Links outside figma.com open in your default browser.

## Children

Ligma is not meant for children under 13, or under the minimum age to use Discord or Figma where you live.

## Changes

If this policy changes, we'll update it here and change the date at the top. The full history is in this repository's commits.

## Contact

Questions or requests: open an issue at [github.com/trelakor/ligma/issues](https://github.com/trelakor/ligma/issues).
