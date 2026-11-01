# Development

## Requirements

- [Rust](https://rustup.rs) (stable)
- Node.js (only for the Tauri CLI)

## Build and run

```sh
npm install
npm run dev     # debug build, opens the app
npm run build   # release bundle in src-tauri/target/release/bundle/
```

Regenerate icons after editing `assets/icon.png` (macOS) or `assets/icon-windows.png` (Windows `icon.ico`):

```sh
npm run icon
```

The social cover, `site/social-cover.png`, is the site's `og:image` and the repo's social preview. It's drawn in `assets/social/cover.html`. After editing it, render it again with a local Chrome or Edge (set `CHROME` to its path if it isn't found), then upload it in the repo's Settings > General > Social preview:

```sh
npm run social-cover
```

## Cleaning up branches and worktrees

Each feature gets its own branch, and often its own worktree. Once a branch's PR is merged, clear out everything that has landed on `main`:

```sh
npm run prune           # lists what would go
npm run prune -- --yes  # removes it
```

This removes worktrees and local branches whose commits are all on `origin/main`, squash merges included. Worktrees with uncommitted changes or unmerged commits stay, as do the main checkout and the worktree you run it from. Build output in `src-tauri/target*/` is ignored, so a separate `--target-dir` won't show up as a change.

## Releasing

1. Bump `version` in `src-tauri/tauri.conf.json`.
2. Add a `## <version> - <date>` section with its changes to [`CHANGELOG.md`](../CHANGELOG.md).
3. If Rust dependencies changed, run `npm run notices` (needs `cargo install --locked cargo-about --features cli`) to refresh [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md), which ships in the installers.
4. Commit the release on `main` as `chore(release): v0.6.0`.
5. Push a matching tag: `git tag v0.6.0 && git push --tags`.

`.github/workflows/release.yml` then:

- stops if the changelog section is missing or the tag doesn't match the version,
- writes the release notes from the changelog section with `scripts/release-notes.mjs` (run it locally to preview),
- builds both platforms, attaches the installers and writes the `latest.json` that installed apps check.

### Update signing

CI signs the update packages with the `TAURI_SIGNING_PRIVATE_KEY` repository secret (a key from `npx tauri signer generate`, without password), whose public key is `plugins.updater.pubkey` in `tauri.conf.json`. Keep the private key: without it, installed apps can't update.
