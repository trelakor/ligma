<div align="center">

<img src="assets/logo.png" width="64" height="64" alt="">

# Ligma

A lightweight desktop app for [Figma](https://www.figma.com), about 4 MB.

[Download](../../releases/latest) · [Features](docs/FEATURES.md) · [Changelog](CHANGELOG.md)

Created in [T3 Code](https://t3.codes).

</div>

## Who it's for

People who use Figma all day and want a desktop app that's lighter than the official one. Ligma is built with [Tauri 2](https://tauri.app) and opens `figma.com` in the system webview (WebView2 on Windows, WKWebView on macOS) instead of bundling a browser engine.

## Features

- **Tabs** like the Figma desktop app, with pinning, emoji icons and reopening your tabs from last time
- **Google sign-in** that works like the desktop app's, alongside it or without it
- **Low memory use:** tabs you aren't looking at are trimmed, frozen and put to sleep ([details](docs/MEMORY.md))
- **Themes** that recolor Figma's interface, plus a light/dark scheme and interface scale
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

- Windows builds haven't been tested on a real Windows machine yet.
- Keyboard tab shortcuts are macOS-only for now.

## Legal

Ligma is [MIT licensed](LICENSE). It's not affiliated with Figma or Discord.

[Terms of Service](TERMS.md) · [Privacy Policy](PRIVACY.md) · [Security](SECURITY.md) · [Third-party notices](THIRD_PARTY_NOTICES.md)
