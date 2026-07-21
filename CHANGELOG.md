# Changelog

All notable changes to LNPM are documented in this file.

## 0.2.0 - 2026-07-21

### Added

- Complete English, Korean, Japanese, Simplified Chinese, and Traditional Chinese localization across the dashboard, settings, popup, tray, notifications, charts, and user-facing errors.
- Locale-aware system language detection, date and number formatting, and instant language switching across all application windows.
- Localized README editions with a centered product hero, language selector, badges, and dashboard screenshot.
- A new cat-ear wireless-signal logo and matching native application icons.

### Changed

- Tauri command and monitor errors now use stable error codes with optional diagnostic details.
- Chart tooltips now stay beside the pointer and flip at chart edges instead of drifting away from the hovered value.
- CI now verifies frontend localization tests alongside the cross-platform TypeScript and Rust checks.

## 0.1.0 - 2026-07-20

### Added

- Cross-platform Tauri 2 tray application written in Rust and TypeScript.
- Concurrent IPv4/IPv6 ICMP monitoring for multiple targets.
- Rolling packet-loss, jitter, P95 latency, outage, and recovery classification.
- SQLite persistence with raw samples, minute rollups, quality intervals, retention, and backups.
- Real-time tray popup and draggable, zoomable historical latency chart.
- Red disconnected intervals and orange unstable intervals with range summaries.
- Native notifications, autostart support, pause/resume, and Korean/English interface.
- GitHub Actions CI and native release packaging for Windows, macOS, and Linux.
