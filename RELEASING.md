# Releasing Afinín

Releases are built by `.github/workflows/release.yml` when a `v*` tag is pushed. It produces
installers for macOS (Apple Silicon and Intel), Linux (AppImage, .deb, .rpm) and Windows
(MSI, NSIS), a signed update manifest (`latest.json`) and CycloneDX SBOMs, all attached to a
**draft** GitHub release. Publish the draft after checking it.

## One-time setup

### Update signing (required)

Auto-updates are verified against a public key compiled into release builds (NFR-6).

```sh
cd apps/desktop
npx tauri signer generate -w ~/.tauri/afinin.key     # choose a strong password
```

In the GitHub repository settings:

| Kind | Name | Value |
|---|---|---|
| Variable | `TAURI_UPDATER_PUBKEY` | contents of `~/.tauri/afinin.key.pub` |
| Secret | `TAURI_SIGNING_PRIVATE_KEY` | contents of `~/.tauri/afinin.key` |
| Secret | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | its password |

Keep the private key offline as well: losing it means existing installs can no longer
auto-update to builds signed with a new key.

### macOS code signing and notarization (recommended)

Without it, macOS shows an "unidentified developer" warning. Set the secrets `APPLE_CERTIFICATE`
(base64 .p12), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`,
`APPLE_PASSWORD` (app-specific password) and `APPLE_TEAM_ID`. See
<https://v2.tauri.app/distribute/sign/macos/>.

### Windows code signing (recommended)

Configure `bundle.windows` signing per <https://v2.tauri.app/distribute/sign/windows/>
(e.g. Azure Trusted Signing) to avoid SmartScreen warnings.

## Cutting a release

1. Update the version in `Cargo.toml` (`[workspace.package]`), `apps/desktop/package.json`
   and `apps/desktop/src-tauri/tauri.conf.json`.
2. Add the changes to `CHANGELOG.md`.
3. Commit with sign-off, tag and push:

   ```sh
   git commit -s -am "release: v0.2.0"
   git tag -s v0.2.0 -m "Afinín v0.2.0"
   git push origin main v0.2.0
   ```

4. Review the draft release (installers, `latest.json`, SBOMs) and publish it. Installed apps
   find the update through `releases/latest/download/latest.json`.

## Local builds

```sh
cd apps/desktop
npm run tauri build                        # no auto-updater
npm run tauri build -- --features updater --config src-tauri/tauri.release.conf.json
```

The second form needs the update key (`TAURI_SIGNING_PRIVATE_KEY`) and a real `pubkey`.
