# Contributing

Thank you for helping improve Live Network Ping Monitor.

## Development workflow

1. Create a focused branch from `main`.
2. Install dependencies with `pnpm install --frozen-lockfile`.
3. Keep changes focused and include tests for Rust behavior.
4. Run the frontend build, Rust tests, formatting check, and Clippy before opening a pull request.
5. Explain the user-visible behavior and platform impact in the pull request.

## Commit style

Use concise conventional-style subjects where practical, for example:

- `feat(ui): add a chart interaction`
- `fix(storage): preserve an open quality interval`
- `docs: clarify Linux prerequisites`

## Platform-specific changes

Tray, notification, autostart, window, and packaging changes should be checked on every affected operating system. GitHub Actions builds all three desktop platforms, but interactive behavior still needs a native smoke test.

## Reporting bugs

Include the operating system and version, LNPM version, monitoring target type (hostname, IPv4, or IPv6), steps to reproduce, and relevant terminal output. Do not attach a database containing sensitive target names or addresses to a public issue.
