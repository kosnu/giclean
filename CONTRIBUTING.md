# 開発手順

## 環境

- Rust / Cargo 1.89.0。`rust-toolchain.toml` で固定しています。
- Git 2.47 以降。macOS / Linux を対象にしています。
- [rustup](https://rustup.rs/) の利用を想定しています。外部クレートの導入は不要です。

`RUSTUP_TOOLCHAIN` や rustup のディレクトリ設定が指定されていると、ツールチェーンファイルより優先されます。使用バージョンは `rustup show active-toolchain` で確認できます。明示する場合は `cargo +1.89.0 test --locked --all-targets` のように指定してください。

## 実行とビルド

```sh
cargo run -- br --dry-run
make fmt    # フォーマット
make check  # フォーマット・Clippy・テスト
make build  # target/release/giclean を生成
```

PATH 上で利用する場合は、`cargo install --path . --locked` で Cargo の bin ディレクトリにインストールします。通常は `~/.cargo/bin` です。

## 検証

`make check` と `make build` は以下を実行します。

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
```

テストは一時的なリポジトリと bare origin を作り、実際の Git コマンドで確認します。Git のグローバル設定とテスト用リポジトリは分離します。外部への通信や既存リポジトリのブランチ削除は行いません。

検証対象は通常のマージ、squash、未マージ、保護対象、複数 worktree、detached HEAD、通信失敗、未取得コミット、照会中の変更、操作直前の変更、部分失敗、SHA-256 リポジトリなどです。

SSH はテスト用の境界からローカルの `git upload-pack` に接続し、共有オプション・明示設定の維持・非対応時の通常接続を確認します。ソケットディレクトリの権限・所有者・シンボリックリンクの検査も対象にします。

破壊的な検証は一時リポジトリだけで行ってください。振る舞いを変更した場合は、対応する結合テストを追加し、`make check` と `make build` を実行します。

GitHub Actions は push / pull request で同じ検証とリリースビルドを macOS / Linux で実行します。CI の設定とリモートでの実行結果は分けて確認してください。

## 配布とドキュメント

公開・配布ライセンスは未設定で、`Cargo.toml` は `publish = false` にしています。

利用開始に必要な情報は [README](README.md)、コマンドの仕様と制約は [br の仕様](docs/commands/br.md)、設計判断や SSH 接続の説明は [設計](docs/design.md)に記載します。
