# Releasing Afinín

Releases are built by `.github/workflows/release.yml`, and only when you start it by hand:
nothing else (pushes, tags, pull requests) triggers it. It produces installers for macOS (Apple
Silicon and Intel), Linux (AppImage, .deb, .rpm) and Windows (MSI, NSIS), a signed update
manifest (`latest.json`) and CycloneDX SBOMs, all attached to a **draft** GitHub release.
Publish the draft after checking it.

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

1. On a branch, set the version in `Cargo.toml` (`[workspace.package]`),
   `apps/desktop/package.json` and `apps/desktop/src-tauri/tauri.conf.json`, and give the
   changes a dated heading in `CHANGELOG.md`, e.g. `## v0.1.0 — 2026-11-02` (a heading that
   still says "Unreleased" is refused). Merge it into `main` and wait for CI to pass.
2. In GitHub: **Actions → Release → Run workflow**, on `main`, with the version (`0.1.0`,
   without the `v`).

   The workflow first checks that the version matches all three files and the CHANGELOG, that
   CI passed on that commit, and that the tag doesn't exist yet. Then it tags the commit
   (`v0.1.0`), builds the four installers in parallel (about 20–30 minutes) and attaches the
   SBOMs.
3. Open the draft release, check the installers, `latest.json` and the SBOMs, edit the notes
   if you like, and **Publish**. Installed apps find the update through
   `releases/latest/download/latest.json` of this repository.

If a build fails, delete the draft release and the tag (`git push origin :refs/tags/v0.1.0`),
fix the cause on `main`, and run the workflow again.

## Local builds

```sh
cd apps/desktop
npm run tauri build                        # no auto-updater
npm run tauri build -- --features updater --config src-tauri/tauri.release.conf.json
```

The second form needs the update key (`TAURI_SIGNING_PRIVATE_KEY`) and a real `pubkey`.
