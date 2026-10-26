# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

People who use Figma all day on macOS or Windows and want a desktop app lighter than the official one. They're designers, so they judge a site by its craft. (Inferred from README.md "Who it's for"; not separately confirmed.)

## Product Purpose

Ligma is a desktop app for Figma, about 4 MB. It opens figma.com in the system webview (WebView2 on Windows, WKWebView on macOS) instead of bundling a browser engine. It adds what the web version lacks: tabs like the desktop app, Google sign-in, low memory use, color themes, plugins in the left rail, development plugins, Discord Rich Presence, local fonts and automatic updates. The landing page succeeds when a visitor downloads it.

## Positioning

It's the same Figma in a much smaller shell: 16 MB installed on macOS against 305 MB for Figma desktop. It also adds extras the official app doesn't have: nine color themes, plugins one click away in the left rail, and your file on Discord.

## Capabilities and Constraints

- Builds aren't signed yet. macOS needs the quarantine flag removed (or Open Anyway). Windows may show SmartScreen.
- Themes: Figma, Grove, Lagoon, Ocean, Midnight, Iris, Rose, Ember and Amber, each with a light and a dark variant (dist/themes.js is the source of truth). They recolor Figma's interface, never your designs.
- Not affiliated with Figma or Discord. MIT licensed.

## Brand Commitments

- Name "Ligma"; the logo is assets/logo.png.
- Voice: plain, factual and understated. Short sentences, no hype (see README.md and docs/FEATURES.md).
- The app is drawn in Figma's own UI3 style (Inter, Figma's grey ramp, blue #0d99ff), and its themes appear as blurred mesh-gradient orbs, one for light and one for dark.

## Evidence on Hand

- Sizes: 2.2 MB Windows download, 7.1 MB macOS download (universal), 16 MB installed on macOS vs 305 MB for Figma desktop.
- No testimonials, user counts or benchmarks beyond the sizes above. Don't invent any.

## Product Principles

- Prove it with the real thing: real sizes, real themes, and real app behavior.
- Stay out of the way: it's still Figma, only lighter.
- Be honest about rough edges, like unsigned builds and unsandboxed dev plugins.
