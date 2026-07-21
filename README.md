<div align="center">
  <img src="docs/assets/lnpm-logo.svg" alt="LNPM logo" width="112" />
  <h1>Live Network Ping Monitor</h1>
  <p>A native tray app for understanding latency, packet loss, and network stability in real time.</p>
  <p>
    <a href="https://github.com/xxsLuna/LNPM/actions/workflows/ci.yml"><img src="https://github.com/xxsLuna/LNPM/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="https://github.com/xxsLuna/LNPM/releases/latest"><img src="https://img.shields.io/github/v/release/xxsLuna/LNPM" alt="Release" /></a>
    <a href="LICENSE"><img src="https://img.shields.io/github/license/xxsLuna/LNPM" alt="License" /></a>
  </p>
  <p>
    <strong>English</strong> ·
    <a href="docs/readme/README.ko.md">한국어</a> ·
    <a href="docs/readme/README.ja.md">日本語</a> ·
    <a href="docs/readme/README.zh-CN.md">简体中文</a> ·
    <a href="docs/readme/README.zh-TW.md">繁體中文</a>
  </p>
  <img src="docs/assets/lnpm-dashboard.png" alt="LNPM live network monitoring dashboard" width="1100" />
</div>

LNPM continuously measures ICMP latency and network quality on Windows, macOS, and Linux. Measurements stay on your device, while real-time and historical charts make intermittent instability and disconnections easy to identify.

## Features

- Monitor multiple hostnames or IPv4/IPv6 addresses simultaneously.
- Keep running in the system tray with a compact live-status popup.
- Browse historical data by dragging or zooming the latency chart.
- Mark disconnected periods in red and unstable periods in orange.
- Calculate average latency, P95 latency, packet loss, jitter, and state percentages.
- Configure per-target packet-loss, jitter, and P95 thresholds.
- Persist raw samples, minute rollups, and quality intervals in a local SQLite database.
- Configure retention, create database backups, and start LNPM at login.
- Receive native notifications for unstable, disconnected, and recovered states.
- Install signed updates from inside LNPM with download progress, deferral, and version skipping.
- Use the complete interface, tray, and notifications in English, Korean, Japanese, Simplified Chinese, or Traditional Chinese.

## Network quality rules

The default classifier uses a rolling 60-second window and starts after 10 samples:

| State | Default rule |
| --- | --- |
| Unstable | Packet loss >= 5%, jitter >= 30 ms, or P95 latency >= 150 ms for 10 seconds |
| Disconnected | Five consecutive failed probes |
| Recovered from unstable | All metrics remain below their thresholds for 30 seconds |
| Recovered from disconnected | Three consecutive successful probes |

The three unstable thresholds can be changed independently for every target.

## Download

Download the appropriate package from [GitHub Releases](https://github.com/xxsLuna/LNPM/releases/latest):

- Windows: NSIS `.exe` or Windows Installer `.msi`
- macOS: `.dmg` for Apple Silicon or Intel
- Linux: `.AppImage` or Debian `.deb`

The initial community builds are not code-signed. Windows SmartScreen or macOS Gatekeeper may therefore show an unknown-publisher warning even when the file was downloaded from this repository.

Updater packages are separately signed with Tauri's updater key and are verified before installation. LNPM v0.1.0 does not contain the updater, so it cannot update itself to v0.2.0. Install v0.2.0 manually once; in-app updates work for releases after v0.2.0.

## Local data

All targets, samples, settings, and rollups are stored in a local SQLite database. LNPM does not upload monitoring data. It only contacts configured probe targets and asks the official Tauri Updater to read the signed `latest.json` from GitHub Releases at startup and every 30 minutes while running.

The exact data directory is available under **Settings -> Data -> Open folder**.

## Development

Prerequisites:

- Rust stable toolchain (Rust 1.85 or newer)
- Node.js 22 or newer
- pnpm 11.9
- [Tauri 2 platform prerequisites](https://v2.tauri.app/start/prerequisites/)

```powershell
pnpm install --frozen-lockfile
pnpm tauri dev
```

Run all local checks:

```powershell
pnpm test
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets --all-features -- -D warnings
```

Docker can run some frontend and Rust checks, but it cannot reliably test the native tray, notifications, ICMP permissions, WebView, or platform installers. Those checks run on native GitHub-hosted Windows, macOS, and Linux runners.

## Release process

Every push and pull request is verified by the cross-platform CI workflow. A version tag such as `v0.2.1` starts the release workflow. The release becomes public only after every platform build succeeds and the workflow verifies the installers, updater signatures, and consolidated `latest.json`.

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines and [SECURITY.md](SECURITY.md) for vulnerability reporting.

## Project administrator

LNPM is maintained and administered by [@xxsLuna](https://github.com/xxsLuna).

## License

Licensed under the [MIT License](LICENSE).
