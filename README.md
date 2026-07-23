# Marginal — Desktop (Windows) build

This turns Marginal into an installed Windows app that:

- installs **without admin** (per-user),
- can be set as your **default PDF reader**,
- **auto-updates** itself when you publish a new version.

You don't install any build tools on your PC — **GitHub builds it for you in the cloud.**

---

## What's in here

```
marginal/
├─ ui/index.html            ← the whole app (self-contained)
├─ package.json
├─ src-tauri/               ← the desktop wrapper (Rust + config)
│  ├─ tauri.conf.json       ← window, PDF association, updater settings
│  ├─ Cargo.toml
│  ├─ build.rs
│  ├─ src/main.rs
│  ├─ src/lib.rs            ← opens the PDF Windows hands us; wires updater
│  ├─ capabilities/default.json
│  └─ icons/                ← app icons (already generated)
└─ .github/workflows/release.yml   ← the cloud build
```

You also received **two key files separately** (keep them private):
`marginal_updater.key` and `marginal_updater.key.pub`. They're already wired in — the
**public** key is in `tauri.conf.json`; the **private** key goes into a GitHub Secret (below).

---

## One‑time setup (about 10 minutes)

### 1. Create the repository
- Make a free account at github.com.
- Click **New repository** → name it exactly **`marginal`** → **Create repository**.

### 2. Upload these files (keep the folder structure)
Easiest: install **GitHub Desktop** (no admin required — it installs per‑user), clone the empty
repo, copy this whole folder into it, then **Commit** and **Push**.
(Or use the web UI: *Add file → Upload files* and drag the folders in.)

> Do **not** upload `marginal_updater.key` / `.pub`. The `.gitignore` already blocks them.

### 3. Point the updater at your repo
Open `src-tauri/tauri.conf.json` and replace `YOUR_GITHUB_USERNAME` in the updater
`endpoints` URL with your GitHub username.

### 4. Add the two signing secrets
In your repo: **Settings → Secrets and variables → Actions → New repository secret.**
Add these two:

| Secret name | Value |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | the **entire contents** of `marginal_updater.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | `marginal-update-2026` |

(These let the cloud build sign each update; the app verifies with the public key.)

### 5. Publish your first version
- Go to **Releases → Draft a new release**.
- **Choose a tag** → type `v1.0.0` → **Create new tag on publish**.
- Title it whatever you like → **Publish release**.
- Open the **Actions** tab. A job runs for ~3–6 minutes and attaches the installer
  (`Marginal_1.0.0_x64-setup.exe`) plus `latest.json` to the release.

### 6. Install it
- Download the `.exe` from the release’s **Assets** and run it. It installs to your user
  folder — **no admin prompt**.
- Open Marginal and click **Set as default** in the header. This opens Windows’ own
  “How do you want to open this file?” dialog — pick **Marginal**, check **Always use this
  app**, click **OK**. That’s it; every PDF now opens in Marginal.

  *(Why a button and not something silent: Windows protects default‑app settings with a
  hidden verification hash so that no app — ours included — can flip your defaults without
  you clicking through its own dialog. This button just opens that dialog directly instead
  of you having to find it in Settings.)*

  If you’d rather do it the “normal” Windows way instead: **Settings → Apps → Default apps →**
  search `.pdf` → choose **Marginal**. Same result.

Now double‑clicking any PDF opens it straight into Marginal.

**Rolling this out to your team:** each person clicks **Set as default** once, themselves,
after installing — it’s a per‑user Windows setting so one person’s choice doesn’t affect
anyone else’s machine, and there’s no way to set it on their behalf without them clicking
through that dialog (that’s Windows’ security model, not a Marginal limitation).

---

## Shipping an update (and how auto‑update works)

The installed app checks your repo’s latest release **every time it launches**. If there’s a
newer version, it downloads and installs it silently, then relaunches. To ship one:

1. Edit whatever you want (usually `ui/index.html`).
2. **Bump the version number** in all three places so the updater knows it’s newer:
   - `src-tauri/tauri.conf.json` → `"version"`
   - `src-tauri/Cargo.toml` → `version`
   - `package.json` → `"version"`
   (e.g. `1.0.0` → `1.0.1`.)
3. Commit and push.
4. **Releases → Draft a new release → tag `v1.0.1` → Publish.**
5. The cloud rebuilds and publishes. Anyone running Marginal gets the update on their next launch.

**“If I just edit the file, does it update?”** No — editing alone does nothing until you
publish a new tagged release (step 4). That’s on purpose, so half‑finished edits don’t ship.

---

## Good to know

- **WebView2:** Marginal uses the Edge WebView2 runtime, which is already on Windows 11 and
  current Windows 10. If a PC lacks it, the installer fetches the per‑user WebView2 bootstrapper
  automatically (still no admin).
- **SmartScreen warning:** because the app isn’t code‑signed with a paid certificate, Windows may
  show a blue “Windows protected your PC” screen on first install/update → click **More info →
  Run anyway**. Buying a code‑signing certificate (~$100–200/yr) removes this; optional.
- **Losing the key:** if `marginal_updater.key` (or its password) is ever lost, you can’t sign
  updates anymore — you'd generate a new key, put the new public key in the config, and ship a
  fresh installer. Keep the key file backed up somewhere safe.
- **The two integration points** most likely to need a small tweak on the first build are in
  `src-tauri/src/lib.rs` (opening the double‑clicked file) and the updater block in
  `ui/index.html` (the `initDesktop()` function). They follow the standard Tauri v2 API; if the
  first build shows an issue there, send me the error and I’ll adjust.
