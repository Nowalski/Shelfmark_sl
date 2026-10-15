# Build and release

## Requirements

- Node.js 22
- Rust stable and Cargo
- The desktop build dependencies for your platform, listed in the [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/)

Install dependencies and start the desktop app:

```sh
npm ci
npm run desktop:dev
```

The browser preview is available with `npm run dev`; it uses fictional data and cannot connect to Firestorm. Build the frontend with `npm run build`, then create an unsigned local package with:

```sh
npx tauri build --ci -c src-tauri/tauri.ci.conf.json
```

The CI configuration builds packages without updater artifacts. Signed updater artifacts are created by the GitHub release workflow.

## Publish a signed release

The release workflow runs when a tag matching `app-v<version>` is pushed. The version must match in `package.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml`.

1. Update all three version fields and the matching package lock entry.
2. Confirm the public updater key in `src-tauri/tauri.conf.json` matches the private key held in the repository’s GitHub Actions secrets.
3. Keep `TAURI_SIGNING_PRIVATE_KEY` and, when applicable, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in GitHub Actions secrets. Never commit either value.
4. Commit the release changes and push the matching tag, for example `app-v0.2.0`.
5. The workflow tests and builds Windows, macOS (Apple silicon and Intel), and Linux packages, then creates a draft release. Review its installers and updater manifest, then publish the draft.

The app checks `https://github.com/Nowalski/Shelfmark_sl/releases/latest/download/latest.json` for signed updates. The browser preview does not use the desktop updater. A new repository does not inherit another repository’s Actions secrets, so the updater key must be configured in the public repository before its first release.

## Project page

The page files are in `site/` and publish through `.github/workflows/pages.yml`. In repository settings, choose **Settings → Pages → GitHub Actions** as the source. Changes to `site/` on `main` then deploy automatically. The workflow can also be run manually from the **Actions** tab.

## License terms

Shelfmark is source-available under the personal-use terms in the repository’s [LICENSE](../LICENSE). Those terms are not an OSI-approved open-source license. See [Contributing](../CONTRIBUTING.md) before proposing changes.
