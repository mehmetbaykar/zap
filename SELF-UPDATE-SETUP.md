# Self-Signed Auto-Update Setup (mehmetbaykar/zap)

This fork is wired so that **your own** GitHub Actions build a Developer ID
signed + notarized DMG, publish it to **your** releases, and your installed Zap
**auto-updates silently** — no manually downloading DMGs.

This document is the *only* part that needs you. Everything in code is already done.

> **Status: ✅ completed 2026-06-12.** The Developer ID Application certificate was
> created (valid until June 2031), all 6 repository secrets are set, and
> notarization authenticates with an App Store Connect API key (no Apple ID
> password / 2FA in CI). The steps below are kept for reference (e.g. cert
> renewal in 2031 or key rotation).

---

## What was already changed in code (for reference)

| Area | File(s) | Change |
|------|---------|--------|
| Update source | `app/src/autoupdate/github.rs` (+ `mac/linux/windows.rs` fallback URLs) | Polls **`mehmetbaykar/zap`** releases instead of `zerx-lab/warp` |
| Team ID | `crates/warp_core/src/macos.rs`, `script/macos/bundle`, `script/Entitlements.plist`, `app/src/persistence/sqlite.rs` | `2BBY89MBSN` → **`P5465LP9FW`** (app group reteam'd to `P5465LP9FW.dev.warp`) |
| Silent update | `app/src/autoupdate/mac.rs`, `mod.rs` | macOS OSS now mounts → **verifies your code signature** → swaps the bundle silently (no Finder drag). If a build is *not* signed by your team, the update is rejected — no corruption. |
| Release CI | `.github/workflows/zap_release.yml` | `--selfsign` (ad-hoc) → `--read-passwords-from-env` (Developer ID sign + notarize + staple) |
| Upstream tracking | `.github/workflows/upstream-watch.yml` | Daily check of `zerx-lab/zap`; opens a sync issue when it moves |

---

## What YOU do (one-time, ~15 min)

### 1. Create a "Developer ID Application" certificate
You already have *Apple Development* + *Apple Distribution*, but **not** Developer
ID (the one needed to notarize a standalone app). You're in the paid program, so
it's free to create:

- Xcode → **Settings → Accounts →** select your team **→ Manage Certificates → + → Developer ID Application**.
  *(or developer.apple.com → Certificates → + → Developer ID Application)*
- ⚠️ **Confirm the Team ID** shown for that cert is **`P5465LP9FW`**. If it's a
  different team, updating it is a one-line constant change.

### 2. Export the cert as `.p12` and base64 it
- Keychain Access → **login** keychain → find **"Developer ID Application: Mehmet Baykar"** → expand it, select **both the cert and its private key** → right-click → **Export 2 items… → .p12**, set an export password.
- Encode it for GitHub:
  ```sh
  base64 -i Developer_ID_Application.p12 | pbcopy
  ```

### 3. Create an App Store Connect API key (for notarization)
- appstoreconnect.apple.com → **Users and Access → Integrations → App Store
  Connect API → +** → download the `AuthKey_<KEYID>.p8` (one-time download) and
  note the **Key ID** and **Issuer ID** from the same page. No 2FA needed in CI.

### 4. Add 6 repository secrets
In **`mehmetbaykar/zap` → Settings → Secrets and variables → Actions → New repository secret**:

| Secret name | Value |
|-------------|-------|
| `DEVELOPER_ID_CERT_P12_BASE64` | the base64 string from step 2 (paste from clipboard) |
| `DEVELOPER_ID_CERT_PASSWORD` | the `.p12` export password from step 2 |
| `CODESIGN_KEYCHAIN_PASSWORD` | any random string (e.g. `openssl rand -hex 16`) |
| `NOTARIZATION_API_KEY_P8_BASE64` | `base64 -i AuthKey_<KEYID>.p8` |
| `NOTARIZATION_KEY_ID` | the Key ID from step 3 |
| `NOTARIZATION_ISSUER_ID` | the Issuer ID from step 3 |

### 5. Fork + push
- Fork `zerx-lab/zap` to `mehmetbaykar/zap` (GitHub UI, or `gh repo fork zerx-lab/zap --clone=false`).
- Push this work: `git push -u origin feat/english-self-update`, then merge into `main` on your fork.

### 6. Cut your first release
- Push a version tag (scheme matches the app, `vYYYY.MM.DD.N`):
  ```sh
  git tag v2026.06.10.1 && git push origin v2026.06.10.1
  ```
- The **"Zap Release"** workflow builds, signs with your Developer ID, notarizes,
  staples, and publishes `Zap-arm64.dmg` / `Zap-intel.dmg` to your Releases.

### 7. Install once
- Download that DMG, drag Zap to `/Applications`, open it once.
- From then on it **auto-updates silently** (polls your releases every 10 min →
  downloads → verifies your signature → swaps the bundle → relaunches).

---

## Keeping up with upstream

The **Watch Upstream** workflow opens a "Upstream sync" issue when `zerx-lab/zap`
gets new commits. Because this fork is English-only it has **diverged**, so syncs
are not auto-merged. To sync:

```sh
git fetch upstream && git merge upstream/main
# resolve conflicts, then re-run the Chinese→English migration on any new Chinese
git push && git tag vYYYY.MM.DD.N && git push origin vYYYY.MM.DD.N
```

The tag triggers a fresh signed release, and your desktop auto-updates.

---

## Notes

- **Watch the first release cycle once.** The silent-swap path is wired and
  reasoned through, but a full *sign → notarize → publish → in-app update* can
  only be proven end-to-end with your real certificate. After it works once, it's
  hands-off.
- **No Fastlane / Sparkle needed.** Warp (and this fork) use a custom Rust
  updater; `script/macos/bundle` already does `codesign` + `xcrun notarytool` +
  `xcrun stapler`. We just turned it on.
- **Local-build alternative.** If you ever prefer building on your Mac instead of
  CI (cert already in your keychain), run:
  `script/bundle --channel oss --arch aarch64 --read-passwords-from-env` with the
  same env vars exported, then upload the DMG to a GitHub release.
- **App Group.** `P5465LP9FW.dev.warp` ideally should be registered for your team
  (Certificates, IDs & Profiles → Identifiers → App Groups). It's optional — the
  code falls back gracefully if the container can't be created.
