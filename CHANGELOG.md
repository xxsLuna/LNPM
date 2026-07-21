# Changelog

All notable changes to LNPM are documented in this file.

## 0.2.1 - 2026-07-21

### Added

- An explicit all-monitors selection with combined live latency, monitor-time metrics, and equal chart emphasis.

### Changed

- The application and native icon set now use a pixel-aligned, symmetric cat-ear wireless mark.
- The all-monitors selector now uses a compact single-line layout that keeps localized labels readable.
- The tray keeps the cat-ear wireless mark and tints it teal, orange, or gray to reflect live network health.
- Native quality alerts are grouped into a single notification when several monitors change state together.
- Project metadata identifies `xxsLuna` as the project administrator and disables generated contributor sections in future release notes.

### Fixed

- Replaced text-like pause and resume glyphs with proper interface icons.
- Kept monitor management controls inside the sidebar without horizontal scrolling.
- Removed the redundant inner frame from the compact tray popup.
- Explicitly embeds the LNPM window icon for Windows title-bar and taskbar use.

## 0.2.0 - 2026-07-21

### Added

- Complete English, Korean, Japanese, Simplified Chinese, and Traditional Chinese localization across the dashboard, settings, popup, tray, notifications, charts, and user-facing errors.
- Locale-aware system language detection, date and number formatting, and instant language switching across all application windows.
- Localized README editions with a centered product hero, language selector, badges, and dashboard screenshot.
- A new cat-ear wireless-signal logo and matching native application icons.
- Signed in-app updates powered by the official Tauri Updater, with startup and 30-minute checks, progress reporting, 24-hour deferral, version skipping, and retryable failures.

### Changed

- Tauri command and monitor errors now use stable error codes with optional diagnostic details.
- Chart tooltips now stay beside the pointer and flip at chart edges instead of drifting away from the hovered value.
- CI now verifies frontend localization tests alongside the cross-platform TypeScript and Rust checks.

### Upgrade note

- LNPM v0.1.0 does not include the updater and cannot update itself. Install v0.2.0 manually once; in-app updates apply to releases after v0.2.0.

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
