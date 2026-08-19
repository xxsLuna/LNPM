# Changelog

All notable changes to LNPM are documented in this file.

## 0.3.0 - 2026-08-19

### Fixed

- The setup dialog no longer appears on an installation that already has monitors. Both windows used to start before the backend had registered its state, so the first query could be rejected and an empty answer was treated as a fresh install; the windows are now created once the state exists, the frontend retries and reports a failure instead of assuming, and the dialog is reserved for a genuinely first run.
- Creating a backup completes instead of hanging the application. The incremental copy restarted from the beginning on every concurrent probe write and never finished, and it ran on the UI thread.
- Time when LNPM was not running is no longer counted as monitored time. Intervals left open by a crash, a kill or an installer restart are closed at the last recorded sample, pausing or disabling a monitor closes its interval, and a gap in observation (system sleep, a stalled network stack) is recorded as unobserved rather than charged to the state before it.
- Retention is enforced. It now runs at startup instead of only after six hours, prunes the minute rollups and closed intervals it used to leave behind, and deletes in time slices so a large prune never blocks the probes.
- Monitors with an interval of seven seconds or more left "warming up" and can now report their quality: the minimum sample count is clamped to what the measurement window can hold.
- Database work no longer runs on the UI thread, so a busy database cannot freeze the windows and the tray.
- Saving a monitor no longer overwrites stored settings with defaults, and a rejected save no longer leaves a stray monitor behind that reappears on every retry.
- The monitor list is no longer rebuilt several times a second, so clicks and keyboard focus survive live updates.
- Fixed the dead "Open data folder" button, chart panning and wheel zoom on scaled displays, the loading overlay that could stay up forever, the chart being rebuilt (and losing zoom and tooltip) on every refresh, and the blank chart left behind after deleting a monitor.
- Windows parked in the tray no longer keep querying history every five seconds.
- Update downloads are bounded by a timeout, the application stays quittable while one is running, an ignored update dialog no longer stops update checks, and a deferred version no longer hides a newer release for a day.

### Changed

- "P95 latency" is the 95th percentile of the samples themselves; it was the 95th percentile of per-minute maxima, which reported a far higher number. Long ranges are answered from the per-minute rollups and say so in the card label.
- The all-monitors view reports the worst monitor for each time metric, with labels that say so, instead of adding monitor time together.
- Recovering from an outage restarts the quality window, so a recovered monitor is not reported as unstable for the following minute.
- New monitors default to a 800 ms timeout so a probe cannot consume its own sampling tick during an outage.
- The tray reports the worst observed monitor instead of falling back to "paused" whenever a single monitor is disabled.
- Cleaning up now reclaims disk space, converting databases that were created before incremental vacuuming was applied correctly.
- A failed startup reports itself through a notification and a log file instead of exiting invisibly.
- Chart tick labels follow the application language.

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
