<div align="center">
  <img src="../assets/lnpm-logo.png" alt="LNPM 標誌" width="112" />
  <h1>Live Network Ping Monitor</h1>
  <p>用於即時掌握延遲、封包遺失與網路穩定性的原生系統匣應用程式。</p>
  <p>
    <a href="https://github.com/xxsLuna/LNPM/actions/workflows/ci.yml"><img src="https://github.com/xxsLuna/LNPM/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="https://github.com/xxsLuna/LNPM/releases/latest"><img src="https://img.shields.io/github/v/release/xxsLuna/LNPM" alt="版本" /></a>
    <a href="../../LICENSE"><img src="https://img.shields.io/github/license/xxsLuna/LNPM" alt="授權條款" /></a>
  </p>
  <p>
    <a href="../../README.md">English</a> ·
    <a href="README.ko.md">한국어</a> ·
    <a href="README.ja.md">日本語</a> ·
    <a href="README.zh-CN.md">简体中文</a> ·
    <strong>繁體中文</strong>
  </p>
  <img src="../assets/lnpm-dashboard.png" alt="LNPM 即時網路監控儀表板" width="1100" />
</div>

LNPM 可在 Windows、macOS 與 Linux 上持續測量 ICMP 延遲和網路品質。測量資料只保存在本機，即時與歷史圖表可協助你輕鬆找出間歇性的網路不穩定與中斷問題。

## 功能

- 同時監控多個主機名稱或 IPv4/IPv6 位址。
- 常駐系統匣，並提供精簡的即時狀態快顯視窗。
- 拖曳或縮放延遲圖表以瀏覽歷史資料。
- 以紅色標記中斷時段，以橘色標記不穩定時段。
- 計算平均延遲、P95 延遲、封包遺失率、抖動與各狀態比例。
- 為每個目標分別設定封包遺失、抖動與 P95 閾值。
- 將原始測量、每分鐘彙總與品質區間保存在本機 SQLite 資料庫中。
- 設定資料保留期、建立資料庫備份及登入時自動啟動。
- 在不穩定、中斷與恢復時接收原生通知。
- 可在 LNPM 內查看進度並安裝已簽署的更新，也可選擇稍後提醒或略過特定版本。
- 介面、系統匣與通知完整支援英文、韓文、日文、簡體中文與繁體中文。

## 網路品質規則

預設分類器使用最近60秒的滾動視窗，並在取得10筆測量樣本後開始判斷。

| 狀態 | 預設規則 |
| --- | --- |
| 不穩定 | 封包遺失率 >= 5%、抖動 >= 30ms 或 P95 延遲 >= 150ms，持續10秒 |
| 已中斷 | 連續5次探測失敗 |
| 從不穩定恢復 | 所有指標持續30秒低於各自閾值 |
| 從中斷恢復 | 連續3次探測成功 |

三個不穩定閾值均可為每個目標分別修改。

## 下載

請從 [GitHub Releases](https://github.com/xxsLuna/LNPM/releases/latest) 下載適合你系統的套件：

- Windows：NSIS `.exe` 或 Windows Installer `.msi`
- macOS：適用於 Apple Silicon 或 Intel 的 `.dmg`
- Linux：`.AppImage` 或 Debian `.deb`

初期社群版本未進行程式碼簽署。因此，即使檔案來自本儲存庫，Windows SmartScreen 或 macOS Gatekeeper 也可能顯示未知發行者警告。

更新套件會使用獨立的 Tauri updater 金鑰簽署，並在安裝前驗證。LNPM v0.1.0 不含 updater，因此無法自動更新至 v0.2.0。手動安裝一次 v0.2.0 後，後續版本即可使用應用程式內更新。

## 本機資料

所有目標、測量值、設定與彙總資料都保存在本機 SQLite 資料庫中。LNPM 不會上傳監控資料，只會連線至已設定的探測目標；官方 Tauri Updater 會在啟動時以及執行期間每30分鐘讀取 GitHub Releases 中已簽署的 `latest.json`。

你可以在 **設定 -> 資料 -> 開啟資料夾** 中查看確切的資料目錄。

## 開發

環境需求：

- Rust 穩定版工具鏈（Rust 1.85 或更新版本）
- Node.js 22 或更新版本
- pnpm 11.9
- [Tauri 2 平台必要元件](https://v2.tauri.app/start/prerequisites/)

```powershell
pnpm install --frozen-lockfile
pnpm tauri dev
```

執行所有本機檢查：

```powershell
pnpm test
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets --all-features -- -D warnings
```

Docker 可以執行部分前端與 Rust 檢查，但無法可靠測試原生系統匣、通知、ICMP 權限、WebView 或平台安裝程式。這些項目會在 GitHub 提供的 Windows、macOS 與 Linux 原生執行器上驗證。

## 發佈流程

每次 push 與 pull request 都會經過跨平台 CI 工作流程驗證。建立 `v0.2.0` 之類的版本標籤後會啟動發佈工作流程。只有所有平台建置成功，並完成安裝檔、updater 簽章和整合 `latest.json` 驗證後，版本才會公開發佈。

貢獻指南請參閱 [CONTRIBUTING.md](../../CONTRIBUTING.md)，安全性漏洞回報請參閱 [SECURITY.md](../../SECURITY.md)。

## 授權條款

本專案採用 [MIT License](../../LICENSE)。
