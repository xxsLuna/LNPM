# Live Network Ping Monitor

[![CI](https://github.com/xxsLuna/LNPM/actions/workflows/ci.yml/badge.svg)](https://github.com/xxsLuna/LNPM/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/xxsLuna/LNPM)](https://github.com/xxsLuna/LNPM/releases/latest)
[![License](https://img.shields.io/github/license/xxsLuna/LNPM)](LICENSE)

Live Network Ping Monitor (LNPM) is a native tray application for continuously measuring ICMP latency and network quality on Windows, macOS, and Linux.

LNPM stores measurements locally, draws real-time and historical latency charts, and highlights unstable or disconnected periods so intermittent network problems are easier to identify.

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
- Use the interface in Korean or English.

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

## Local data

All targets, samples, settings, and rollups are stored in a local SQLite database. LNPM does not upload monitoring data. It only contacts configured probe targets and checks the public GitHub Releases API once per day for updates.

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
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets --all-features -- -D warnings
```

Docker can run some frontend and Rust checks, but it cannot reliably test the native tray, notifications, ICMP permissions, WebView, or platform installers. Those checks run on native GitHub-hosted Windows, macOS, and Linux runners.

## Release process

Every push and pull request is verified by the cross-platform CI workflow. A version tag such as `v0.1.0` starts the release workflow and publishes the native packages after every platform build succeeds.

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines and [SECURITY.md](SECURITY.md) for vulnerability reporting.

## License

Licensed under the [MIT License](LICENSE).
