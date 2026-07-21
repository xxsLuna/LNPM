<div align="center">
  <img src="../assets/lnpm-logo.svg" alt="LNPM 标志" width="112" />
  <h1>Live Network Ping Monitor</h1>
  <p>用于实时了解延迟、丢包和网络稳定性的原生托盘应用。</p>
  <p>
    <a href="https://github.com/xxsLuna/LNPM/actions/workflows/ci.yml"><img src="https://github.com/xxsLuna/LNPM/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="https://github.com/xxsLuna/LNPM/releases/latest"><img src="https://img.shields.io/github/v/release/xxsLuna/LNPM" alt="版本" /></a>
    <a href="../../LICENSE"><img src="https://img.shields.io/github/license/xxsLuna/LNPM" alt="许可证" /></a>
  </p>
  <p>
    <a href="../../README.md">English</a> ·
    <a href="README.ko.md">한국어</a> ·
    <a href="README.ja.md">日本語</a> ·
    <strong>简体中文</strong> ·
    <a href="README.zh-TW.md">繁體中文</a>
  </p>
  <img src="../assets/lnpm-dashboard.png" alt="LNPM 实时网络监控仪表板" width="1100" />
</div>

LNPM 可在 Windows、macOS 和 Linux 上持续测量 ICMP 延迟和网络质量。测量数据仅保存在本机，实时与历史图表可帮助你轻松发现间歇性网络不稳定和断开问题。

## 功能

- 同时监控多个主机名或 IPv4/IPv6 地址。
- 常驻系统托盘，并提供简洁的实时状态弹窗。
- 拖动或缩放延迟图表以浏览历史数据。
- 用红色标记断开时段，用橙色标记不稳定时段。
- 计算平均延迟、P95 延迟、丢包率、抖动和各状态占比。
- 为每个目标分别配置丢包、抖动和 P95 阈值。
- 将原始测量、分钟汇总和质量区间保存在本地 SQLite 数据库中。
- 配置数据保留期、创建数据库备份以及登录时自动启动。
- 在不稳定、断开和恢复时接收原生通知。
- 可在 LNPM 内查看进度并安装已签名的更新，也可选择稍后提醒或跳过特定版本。
- 界面、托盘和通知完整支持英语、韩语、日语、简体中文和繁体中文。

## 网络质量规则

默认分类器使用最近60秒的滚动窗口，并在获得10个测量样本后开始判断。

| 状态 | 默认规则 |
| --- | --- |
| 不稳定 | 丢包率 >= 5%、抖动 >= 30ms 或 P95 延迟 >= 150ms，持续10秒 |
| 已断开 | 连续5次探测失败 |
| 从不稳定恢复 | 所有指标持续30秒低于各自阈值 |
| 从断开恢复 | 连续3次探测成功 |

三个不稳定阈值均可为每个目标单独修改。

## 下载

请从 [GitHub Releases](https://github.com/xxsLuna/LNPM/releases/latest) 下载适合你系统的软件包：

- Windows：NSIS `.exe` 或 Windows Installer `.msi`
- macOS：适用于 Apple Silicon 或 Intel 的 `.dmg`
- Linux：`.AppImage` 或 Debian `.deb`

初期社区构建未进行代码签名。因此，即使文件来自本仓库，Windows SmartScreen 或 macOS Gatekeeper 也可能显示未知发布者警告。

更新包会使用独立的 Tauri updater 密钥签名，并在安装前验证。LNPM v0.1.0 不包含 updater，因此无法自动更新到 v0.2.0。手动安装一次 v0.2.0 后，后续版本即可使用应用内更新。

## 本地数据

所有目标、测量值、设置和汇总数据均保存在本地 SQLite 数据库中。LNPM 不会上传监控数据，只会连接已配置的探测目标；官方 Tauri Updater 会在启动时以及运行期间每30分钟读取 GitHub Releases 中已签名的 `latest.json`。

你可以在 **设置 -> 数据 -> 打开文件夹** 中查看准确的数据目录。

## 开发

环境要求：

- Rust 稳定版工具链（Rust 1.85 或更高版本）
- Node.js 22 或更高版本
- pnpm 11.9
- [Tauri 2 平台依赖](https://v2.tauri.app/start/prerequisites/)

```powershell
pnpm install --frozen-lockfile
pnpm tauri dev
```

运行全部本地检查：

```powershell
pnpm test
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets --all-features -- -D warnings
```

Docker 可以运行部分前端和 Rust 检查，但无法可靠测试原生托盘、通知、ICMP 权限、WebView 或平台安装包。这些项目会在 GitHub 提供的 Windows、macOS 和 Linux 原生运行器上验证。

## 发布流程

每次 push 和 pull request 都会通过跨平台 CI 工作流验证。创建 `v0.2.1` 之类的版本标签后会启动发布工作流。只有所有平台构建成功，并完成安装包、updater 签名和统一 `latest.json` 验证后，版本才会公开发布。

有关贡献指南，请参阅 [CONTRIBUTING.md](../../CONTRIBUTING.md)；有关安全漏洞报告，请参阅 [SECURITY.md](../../SECURITY.md)。

## 项目管理员

LNPM 由项目管理员 [@xxsLuna](https://github.com/xxsLuna) 维护和管理。

## 许可证

本项目采用 [MIT License](../../LICENSE)。
