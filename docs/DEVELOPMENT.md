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
