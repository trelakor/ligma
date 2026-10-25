<div align="center">

<img src="assets/logo.png" width="64" height="64" alt="">

# Ligma

A lightweight desktop app for [Figma](https://www.figma.com), about 4 MB.

[Download](../../releases/latest) · [Features](docs/FEATURES.md) · [Changelog](CHANGELOG.md)

</div>

## Who it's for

People who use Figma all day and want a desktop app that's lighter than the official one. Ligma is built with [Tauri 2](https://tauri.app) and opens `figma.com` in the system webview (WebView2 on Windows, WKWebView on macOS) instead of bundling a browser engine.

It also adds touches the official app doesn't have: color themes to make Figma your own, your plugins one click away in the editor's left rail, and your current file on your Discord profile.

## Features

- **Tabs** like the Figma desktop app, with pinning, emoji icons and reopening your tabs from last time
- **Google sign-in** that works like the desktop app's, alongside it or without it
- **Low memory use:** tabs you aren't looking at are trimmed, frozen and put to sleep ([details](docs/MEMORY.md))
- **Themes** in light and dark variants that recolor Figma's interface, with an adjustable interface scale
- **Plugins in the left rail:** your saved and development plugins under Figma's own buttons, one click to run
- **Development plugins** imported from a `manifest.json`, which figma.com doesn't support
- **Discord Rich Presence** showing your file, page and tool, with optional invites to files
- **Local fonts** through Figma's font helper
- **Automatic updates** in the background, without signing you out

See [Features](docs/FEATURES.md) for how each one works.

## Install

Download the latest build for macOS or Windows from [Releases](../../releases/latest). Builds are unsigned; the release notes explain how to open them. After that, Ligma updates itself.

To report a problem, use **Report…** in Settings (or **Help > Report a Problem…** on macOS).

## Build from source

Needs [Rust](https://rustup.rs) (stable) and Node.js.

```sh
npm install
npm run dev     # debug build, opens the app
npm run build   # release bundle
```

See [Development](docs/DEVELOPMENT.md) for icons and releasing.

## Known limits

- Keyboard tab shortcuts (`Cmd+T`, `Cmd+W`, `Ctrl+Tab`) are macOS-only for now.

## Legal

Ligma is [MIT licensed](LICENSE). It's not affiliated with Figma or Discord.

[Terms of Service](TERMS.md) · [Privacy Policy](PRIVACY.md) · [Security](SECURITY.md) · [Third-party notices](THIRD_PARTY_NOTICES.md)

---

<sub>Created in [T3 Code](https://t3.codes).</sub>
