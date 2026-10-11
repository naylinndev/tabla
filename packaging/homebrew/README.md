# Homebrew tap setup (one time)

1. Create an empty public repo named **`homebrew-tabla`** under the same GitHub account.
2. Create a fine-grained personal access token with **Contents: Read and write**
   on that repo only.
3. In this repo, go to **Settings → Secrets and variables → Actions** and add it
   as **`HOMEBREW_TAP_TOKEN`**.
4. Tag a release: `git tag v0.1.0 && git push origin v0.1.0`.

The release workflow builds the DMGs, attaches them to a draft release, and
writes `Casks/tabla.rb` into the tap. After you publish the release:

```bash
brew install --cask naylinndev/tabla/tabla
```

## Signing (recommended)

Without an Apple Developer ID the app is unsigned and macOS Gatekeeper warns on
first launch. To sign and notarize, add these repository secrets; the workflow
picks them up automatically:

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of your Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | password of that `.p12` |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` / `APPLE_PASSWORD` | Apple ID and an app-specific password, for notarization |
| `APPLE_TEAM_ID` | your 10-character team ID |

## One-click updates (recommended)

Release builds can download, verify and install new versions themselves
(**Install and restart** in the update banner). Updates are signed with a key
only you hold; the app refuses anything not signed by it. To switch it on once:

1. Generate a key pair on your own machine (keep the password safe):

   ```bash
   pnpm tauri signer generate -w ~/.tauri/tabla-updater.key
   ```

2. In this repo, go to **Settings → Secrets and variables → Actions**:
   - **Secrets:** add `TAURI_SIGNING_PRIVATE_KEY` (the contents of
     `~/.tauri/tabla-updater.key`) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
   - **Variables:** add `TAURI_UPDATER_PUBKEY` (the contents of
     `~/.tauri/tabla-updater.key.pub`).
3. Tag the next release. The workflow signs the update bundles, attaches
   `latest.json`, and builds the public key into the app.

Builds made before the key existed (or from source) keep the "Download"
button, which opens the release page. Don't lose the private key: releases
signed with a new key won't install over builds that trust the old one.
On Linux only the AppImage updates itself; `.deb` installs show Download.
