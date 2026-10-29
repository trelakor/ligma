# AGENTS.md

## What this is

Ligma is a lightweight desktop app for Figma on macOS and Windows, built with Tauri 2. It opens figma.com in the system webview (WKWebView on macOS, WebView2 on Windows) instead of bundling a browser engine, and adds tabs, Google sign-in, low memory use, color themes, plugins in the left rail, development plugins and Discord Rich Presence.

- `src-tauri/src/` is the app, one module per feature: `tabs.rs` the tabs, `auth.rs` sign-in, `appearance.rs` themes, `plugins.rs` the left rail and development plugins, `presence.rs` and `discord_ipc.rs` Discord, `updater.rs` updates
- `wkwebview.rs`, `titlebar.rs`, `pressure.rs` and `web_auth.rs` are macOS only, `webview2.rs` and `frame.rs` are Windows only
- `dist/` holds Ligma's own pages: the tab bar (`index.html`), settings, menus, tab previews and prompts. `dist/themes.js` is the source of truth for themes
- `site/` is the project website, published to GitHub Pages from `main`
- `scripts/` holds the release notes, Discord asset and memory measuring scripts, `docs/` the feature, memory and development docs
- `PRODUCT.md` describes the product, its users and its voice, read it before UI or copy changes
- Every release needs a `## <version> - <date>` section in `CHANGELOG.md`, the release workflow fails without it. See `docs/DEVELOPMENT.md` for releasing
- Both platforms are supported, keep `cfg(target_os = ...)` paths in step and don't break the other platform's build

## Conventions

- Comment only when it helps contributors understand non-obvious behavior
- Use scoped Conventional Commits, e.g. `fix(tabs): ...`, `feat(plugins): ...`, `chore(site): ...`
- Release commits are `chore(release): vX.Y.Z`
- Keep commit messages to the subject line, avoid commit bodies unless absolutely necessary, and keep them very short when used
- Never add co-author trailers, "Generated with" lines or any other AI or agent attribution to commits, PRs, issues or files. Commits and PRs carry no sign of an agent

## Branching

- Every feature or fix goes on its own branch, never straight onto `main`
- Before the first edit, branch from an up-to-date `main`: `git switch main && git pull && git switch -c feature/<short-slug>` (`fix/<short-slug>` for bug fixes)
- If you're already on a branch for the same work, keep using it. If it's for different work, ask before switching
- Commit to the branch and push it with `git push -u origin <branch>`. Open a PR against `main` only when asked, and don't merge it yourself
- Release commits and tags are the one exception: they're made on `main` after the feature branch has been merged

## Text Conventions

- Do not put periods at the end of comments
- Do not put periods at the end of user-facing strings, unless the string is more than one sentence long
