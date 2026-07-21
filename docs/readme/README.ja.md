<div align="center">
  <img src="../assets/lnpm-logo.svg" alt="LNPM ロゴ" width="112" />
  <h1>Live Network Ping Monitor</h1>
  <p>遅延、パケットロス、ネットワークの安定性をリアルタイムで把握するネイティブトレイアプリです。</p>
  <p>
    <a href="https://github.com/xxsLuna/LNPM/actions/workflows/ci.yml"><img src="https://github.com/xxsLuna/LNPM/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="https://github.com/xxsLuna/LNPM/releases/latest"><img src="https://img.shields.io/github/v/release/xxsLuna/LNPM" alt="リリース" /></a>
    <a href="../../LICENSE"><img src="https://img.shields.io/github/license/xxsLuna/LNPM" alt="ライセンス" /></a>
  </p>
  <p>
    <a href="../../README.md">English</a> ·
    <a href="README.ko.md">한국어</a> ·
    <strong>日本語</strong> ·
    <a href="README.zh-CN.md">简体中文</a> ·
    <a href="README.zh-TW.md">繁體中文</a>
  </p>
  <img src="../assets/lnpm-dashboard.png" alt="LNPM リアルタイムネットワーク監視ダッシュボード" width="1100" />
</div>

LNPM は Windows、macOS、Linux で ICMP 遅延とネットワーク品質を継続的に測定します。測定データは端末内だけに保存され、リアルタイムと履歴のチャートから断続的な不安定や切断を簡単に確認できます。

## 主な機能

- 複数のホスト名または IPv4/IPv6 アドレスを同時に監視します。
- システムトレイで常駐し、コンパクトなライブステータスポップアップを表示します。
- 遅延チャートをドラッグまたはズームして履歴を確認します。
- 切断期間を赤、不安定期間をオレンジで表示します。
- 平均遅延、P95 遅延、パケットロス、ジッター、状態別割合を計算します。
- 対象ごとにパケットロス、ジッター、P95 のしきい値を設定します。
- 生データ、1分単位の集計、品質区間をローカル SQLite データベースに保存します。
- 保存期間、データベースのバックアップ、ログイン時の自動起動を設定します。
- 不安定、切断、復旧時にネイティブ通知を受け取ります。
- LNPM 内で署名済みアップデートを進捗表示付きでインストールし、後で通知または特定バージョンのスキップを選択できます。
- 画面、トレイ、通知のすべてを英語、韓国語、日本語、簡体字中国語、繁体字中国語で利用できます。

## ネットワーク品質の判定ルール

既定の分類器は直近60秒の範囲を使用し、10件の測定値が集まると判定を開始します。

| 状態 | 既定のルール |
| --- | --- |
| 不安定 | パケットロス >= 5%、ジッター >= 30ms、または P95 遅延 >= 150ms が10秒間継続 |
| 切断 | 5回連続で測定に失敗 |
| 不安定から復旧 | すべての指標が30秒間しきい値を下回る |
| 切断から復旧 | 3回連続で測定に成功 |

3つの不安定判定しきい値は対象ごとに個別設定できます。

## ダウンロード

[GitHub Releases](https://github.com/xxsLuna/LNPM/releases/latest) から環境に合うパッケージをダウンロードしてください。

- Windows: NSIS `.exe` または Windows Installer `.msi`
- macOS: Apple Silicon または Intel 用 `.dmg`
- Linux: `.AppImage` または Debian `.deb`

初期のコミュニティビルドはコード署名されていません。このリポジトリから入手したファイルでも、Windows SmartScreen や macOS Gatekeeper が発行元不明の警告を表示することがあります。

アップデートパッケージは別途 Tauri updater キーで署名され、インストール前に検証されます。LNPM v0.1.0 には updater が含まれないため、v0.2.0 へ自動更新できません。v0.2.0 を一度手動でインストールすると、それ以降のリリースではアプリ内更新が利用できます。

## ローカルデータ

対象、測定値、設定、集計はすべてローカル SQLite データベースに保存されます。LNPM は監視データをアップロードしません。設定された測定先へ通信し、公式 Tauri Updater が起動時と実行中30分ごとに GitHub Releases の署名済み `latest.json` を確認します。

正確なデータフォルダーは **設定 -> データ -> フォルダーを開く** で確認できます。

## 開発

必要な環境:

- Rust 安定版ツールチェーン（Rust 1.85 以降）
- Node.js 22 以降
- pnpm 11.9
- [Tauri 2 のプラットフォーム要件](https://v2.tauri.app/start/prerequisites/)

```powershell
pnpm install --frozen-lockfile
pnpm tauri dev
```

すべてのローカルチェックを実行します。

```powershell
pnpm test
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets --all-features -- -D warnings
```

Docker では一部のフロントエンドと Rust のチェックを実行できますが、ネイティブトレイ、通知、ICMP 権限、WebView、各プラットフォームのインストーラーは確実にテストできません。これらは GitHub が提供する Windows、macOS、Linux のネイティブランナーで検証します。

## リリース手順

すべての push と pull request はクロスプラットフォーム CI で検証されます。`v0.2.1` のようなバージョンタグを作成するとリリースワークフローが開始します。全プラットフォームのビルドが成功し、インストーラー、updater 署名、統合 `latest.json` の検証が完了した後にのみリリースを公開します。

コントリビューションについては [CONTRIBUTING.md](../../CONTRIBUTING.md)、脆弱性の報告については [SECURITY.md](../../SECURITY.md) を参照してください。

## プロジェクト管理者

LNPM はプロジェクト管理者 [@xxsLuna](https://github.com/xxsLuna) が保守・管理しています。

## ライセンス

[MIT License](../../LICENSE) の下で提供されます。
