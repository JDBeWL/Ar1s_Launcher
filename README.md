English | [简体中文](README.zh-CN.md)

# Ar1s Launcher

**A modern Minecraft launcher built with Tauri 2 and Vue 3, with a Material Design 3 interface.**

Ar1s Launcher manages Minecraft versions, mod loaders and isolated game instances in one desktop app, and installs modpacks without making the player assemble a game directory by hand.

![Version](https://img.shields.io/badge/version-1.0.0-blue)
![Tauri](https://img.shields.io/badge/Tauri-2.12-orange)
![Vue](https://img.shields.io/badge/Vue-3.5-green)
![Rust](https://img.shields.io/badge/Rust-1.95+-red)
![License](https://img.shields.io/badge/license-MIT-blue)

> Status: early development. Windows is the tested platform; macOS and Linux builds are possible but untested. Bug reports are welcome.

## Features

### Versions and instances

- Browse and install Minecraft releases and snapshots
- Independent instances with their own mods, configs, saves and resource packs (optional version isolation)
- Create, rename, delete instances, or open an instance folder in one click
- Forge, Fabric, Quilt and NeoForge installation, including the libraries each loader needs

### Modpacks

- Search and install modpacks from Modrinth
- Filter by game version, loader and category
- Concurrent mod downloads with per-file progress, cancellation and rollback on failure
- CurseForge modpack support is in development (see [Roadmap](#roadmap))

### Mods

- Browse and install Modrinth mods directly into an instance, filtered by that
  instance's Minecraft version and loader
- Required dependencies are resolved and installed automatically
- Update checks are content-based: local jars are identified by SHA-1, so renamed
  files are still recognised, and batch-updating keeps disabled mods disabled
- Enable, disable and delete mods without touching the files by hand

### Resource and shader packs

- Resource packs: browse Modrinth by the instance's game version, enable/disable through `options.txt`, import local files, delete, and check updates by SHA-1
- Shader packs: browse Modrinth, install and enable in one click, and manage the game's own selection
  (`iris.properties` / `oculus.properties` for Iris and Oculus, `optionsshaders.txt` for OptiFine),
  including the Iris/Oculus master switch and SHA-1 based update checks
- Packs without a `shaders` directory are flagged before the game silently refuses to load them

### Downloads

- Automatic source selection, or prefer the official source / a domestic mirror; all modes fall back automatically
- Automatic fallback between sources
- Configurable download threads and resume support
- SHA-1 verification, file integrity checks and one-click repair of missing files

### Accounts and launching

- Microsoft account login (device code flow and authorization code flow with PKCE), with automatic token refresh
- Offline mode, switchable at any time
- Automatic Java detection and per-version matching, with a diagnostic report when no suitable runtime is found
- JVM memory presets, GC strategy and advanced JVM flags
- Known-incompatible combinations (for example, old Forge builds on new Java versions) are blocked before launch instead of failing midway

### Interface language

- Chinese (Simplified) and English, switchable at any time in **Settings → Interface language**;
  no restart needed
- The choice is stored as `uiLanguage` in the config file, so it survives reinstalls of the frontend
  and is shared by every launcher window
- The first launch follows the OS language, falling back to Chinese
- The launcher UI language is independent of the in-game language, which is still set separately
- Vuetify's own component strings (pagination, empty selects, date pickers) are translated through
  the same catalog, so no built-in English leaks into the Chinese UI

### Reliability

- Crash diagnosis: log patterns are classified and rendered as a copyable plain-text report
- Structured logging, one file per launch, last 30 kept
- HTTP proxy support for all outbound requests
- All data stays local; no account data leaves the machine except the requests required for authentication and downloads

## Tech stack

| Layer    | Technologies                                                                          |
| -------- | ------------------------------------------------------------------------------------- |
| Frontend | Vue 3.5 (Composition API), Vuetify 4.3, TypeScript 6.0, Pinia 4, Vue Router 5, Vite 8 |
| Backend  | Rust 1.95+, Tauri 2.12, Tokio, Reqwest, Serde                                         |
| Tooling  | pnpm, Cargo, vue-tsc, Vitest                                                          |

## Requirements

- Node.js 22.12+
- pnpm
- Rust 1.95+ with Cargo
- Tauri system dependencies for your platform (WebView2 on Windows)

## Getting started

```bash
git clone https://github.com/JDBeWL/ar1s_launcher.git
cd ar1s_launcher
pnpm install
pnpm run tauri dev
```

The first Rust build takes a few minutes. Subsequent builds are incremental.

## Building

```bash
pnpm run build        # type-check and build the frontend
pnpm run tauri build  # produce the desktop bundles
```

Useful checks:

```bash
pnpm run type-check
pnpm lint             # oxlint：类型之外的错误写法与 Vue 模板问题
pnpm test
pnpm version:check    # 校验三处版本号与 README badge 是否一致
cargo test --manifest-path src-tauri/Cargo.toml
```

The version lives in `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` and the
README badges. `package.json` is the single source of truth: bump it there, then run
`pnpm version:sync` to update the rest. CI fails on any drift.

Configuration, logs, login credentials and the default `.minecraft` directory live next to the executable, so upgrading means replacing the executable. If you redistribute a build, include `LICENSE` with it.

## Project structure

```
ar1s_launcher/
├─ src/                     # Vue 3 frontend
│  ├─ components/           # UI components (add-instance, instance, settings, ...)
│  ├─ composables/          # reusable composition functions
│  ├─ i18n/                 # vue-i18n setup and locale catalogs (zh-CN, en-US)
│  ├─ stores/               # Pinia stores (download, launcher, notifications, settings, uiLanguage)
│  ├─ views/                # routed pages (home, download, add instance, modpack install, settings)
│  ├─ services/             # Tauri command wrappers and API clients
│  ├─ types/                # TypeScript type definitions
│  ├─ router/               # route configuration
│  └─ plugins/              # Vuetify and other plugins
├─ src-tauri/               # Rust backend
│  └─ src/
│     ├─ controllers/       # #[tauri::command] entry points
│     ├─ services/          # download, launcher, loaders, modpacks, config, Java, auth
│     ├─ models/            # serializable data models
│     ├─ utils/             # file and logging helpers
│     └─ errors/            # unified error type
└─ public/icons/            # application and UI assets
```

## Usage

### Home

Select an installed version, set a player name, choose offline or Microsoft identity, and launch. Shortcuts to downloads, instance creation and instance management sit on the same page.

### Add instance

- **Custom install**: pick a Minecraft version, optionally a loader and loader version, name the instance, create it.
- **Online install**: pick a modpack platform, search and filter, choose a pack version, install. Mods, overrides and the required loader are resolved automatically.

### Instance list

Cards show the loader, game version, mod count and last launch time. The card menu
offers launch, open folder, instance details, diagnostics export, rename and delete.

### Instance detail

Opening an instance goes to its own page instead of a dialog, with tabs for:

- **Mods** — installed mods with enable/disable, delete, install from Modrinth
  (dependencies included) and update checks
- **Game settings** — instance summary, plus a link to the global settings
- **Resource packs** — installed packs with icons and metadata, enable/disable and
  ordering via `options.txt`, delete, import from local files, install from Modrinth
  and SHA-1 based update checks
- **Shaders** — installed shader packs with validity checks, one-click enable through
  the game's own config (`iris.properties` / `oculus.properties` for Iris and Oculus,
  `optionsshaders.txt` for OptiFine), the Iris/Oculus master switch, delete, import
  from local files, install from Modrinth and SHA-1 based update checks

### Downloads

Browse releases and snapshots, filter by type, choose a source strategy (auto, prefer official, or prefer mirror), and follow progress in real time.

### Settings

General (game directory, download source, threads, proxy, interface language, in-game language), account, Java, memory, window, and advanced JVM options.

## Supported mod loaders

| Loader   | Status    | Notes                       |
| -------- | --------- | --------------------------- |
| Forge    | Supported | The most widely used loader |
| Fabric   | Supported | Lightweight loader          |
| Quilt    | Supported | Fabric fork                 |
| NeoForge | Supported | Modern Forge fork           |

## Supported modpack platforms

| Platform   | Status         | Notes                          |
| ---------- | -------------- | ------------------------------ |
| Modrinth   | Supported      | Open modpack platform          |
| CurseForge | In development | Requires CurseForge API access |

## Third-party services and content

- **Modrinth**: pack metadata and files come from the official Modrinth API and CDN.
- **CurseForge** (planned): the launcher will use a user-supplied API key and call the official API. Files will always be downloaded from CurseForge's own CDN — nothing is re-hosted, mirrored or redistributed. Projects that have third-party distribution disabled will not be installed automatically; the launcher will explain why and link to the project page instead.
- **Minecraft**: version manifests, libraries and assets are fetched with automatic selection, preferring the official source or a domestic mirror (BMCLAPI / MC Mirror).
- Metadata is used only to render the interface, is kept in memory or a short-lived local cache, and is never exposed as a public API or resold.
- The launcher is free, contains no ads, and does not attempt to bypass any platform's client or author reward program.

## Troubleshooting

**The game fails to launch** — read the diagnostic report in the failure dialog first. It classifies the log and states the likely cause. Then check the Java configuration (automatic matching is the default) and run the file integrity check.

**Downloads are slow** — switch the download source under Settings → General, check the network, and adjust the thread count.

**A loader installation fails** — make sure the base version is fully downloaded, the network is reachable, and check `logs/` for the detailed error.

**A modpack installation fails** — verify free disk space and network connectivity, then retry. Failed installs are rolled back, so a retry starts clean.

**UI issues** — restart the app and update the system graphics driver.

Logs are written to `logs/` next to the executable.

## Contributing

1. Fork the repository.
2. Create a branch: `git checkout -b feature/my-feature`.
3. Commit your changes.
4. Push the branch and open a pull request.

Please follow the existing code style, add comments where the intent is not obvious, and make sure `pnpm run type-check` and `pnpm test` pass. For bug reports, include reproduction steps and the relevant log excerpt.

## Security

Please report vulnerabilities privately through GitHub's _Report a vulnerability_
rather than in a public issue. [SECURITY.md](SECURITY.md) documents the secrets
policy: API keys are never committed, never compiled into the binary, and never
exposed to the frontend.

## License

MIT — see [LICENSE](LICENSE). Attribution and licenses for third-party assets and icons are recorded in the "Third-party notices" section of the same file.

## Acknowledgements

- [Tauri](https://tauri.app/) — cross-platform application framework
- [Vuetify](https://vuetifyjs.com/) — Material Design component library
- [Material Design Icons](https://pictogrammers.com/library/mdi/) — icon font
- [BMCLAPI](https://bmclapidoc.bangbang93.com/) — Minecraft download mirror
- [Modrinth](https://modrinth.com/) — open modpack and mod platform

Thanks to [M-Crowe](https://github.com/m-crowe) for testing and for parts of the non-MD3 UI design, and to [XuanYuSLF](https://github.com/XuanYuSLF) for the block icon assets in `public/icons` (`dirt_path`, `grass_block`, `snow_grass`).

## Roadmap

- [x] Mod installation and updates (search, dependency resolution, SHA-1 based update checks)
- [x] Resource pack management (local management, Modrinth installs, update checks)
- [x] Shader pack management (local management, Modrinth installs, update checks)
- [x] Internationalization (Chinese and English UI, switchable at runtime)

## Disclaimer

Ar1s Launcher is an unofficial, community project. It is not an official Minecraft product, is not approved by or associated with Mojang or Microsoft, and is not affiliated with or endorsed by CurseForge, Overwolf or Modrinth. All trademarks belong to their respective owners.

---

**Made with ❤️ by JDBeWL**
