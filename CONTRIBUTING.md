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

squash は複数コミットの追加・変更・削除・名前変更・バイナリ・権限・シンボリックリンク・gitlink の完全一致を確認します。一部だけの取り込み、取り込み後の変更や取り消し、共通祖先のない履歴、判定失敗時の削除中止も検証します。

一括判定では、SHA-256 の130先端でバッチ境界と大きな差分出力を通し、同じ先端・ツリーの共有、異なる判定結果の混在、共通祖先が複数ある場合、要求と合わない応答での削除中止を確認します。Git の起動回数とリモート照会回数も検査します。

ツリー ID が同一の場合は、SHA-1 / SHA-256 の両方で差分コマンドを起動せず同じ候補を得られることを確認します。既定ブランチが共通祖先と同一ツリーの場合、先端と同一ツリーの場合、正味の変更がない場合を含め、dry-run と実際の削除を検証します。

SSH はテスト用の境界からローカルの `git upload-pack` に接続し、共有オプション・明示設定の維持・非対応時の通常接続を確認します。ソケットディレクトリの権限・所有者・シンボリックリンクの検査も対象にします。

既定のプロトコル v0 と、リポジトリ設定・コマンドスコープの環境変数で指定した v2 で、SHA-1 / SHA-256 の候補が一致することも確認します。PATH 上の Git ラッパーを通す検証は macOS でも行い、Apple の Git 本体解決が利用者のラッパーを迂回しないことを検査します。

削除の一括処理は、SHA-1 / SHA-256・日本語名・保護対象・ブランチ設定の後片付けと Git 起動回数を検査します。削除直前の先端更新、同じ OID の symbolic ref 化、Git の初期検査後に成功した worktree 作成、検査コマンドの失敗・応答不正、既存フックの拒否、参照ロック競合、参照削除後のエラーも実リポジトリで再現します。dry-run、129本の候補、一時領域の準備失敗では個別処理の動作を確認します。

進捗表示は macOS / Linux 標準の `script` コマンドで疑似端末を作り、ローカル処理中のスピナー更新、リモート照会中の再描画停止、完了・失敗表示、Git の診断出力、`TERM=dumb` での表示を確認します。実際の `git credential fill` にダミーの認証情報を入力し、`/dev/tty` に出る案内が入力待ちの間に上書きされないことも検証します。

破壊的な検証は一時リポジトリだけで行ってください。振る舞いを変更した場合は、対応する結合テストを追加し、`make check` と `make build` を実行します。

GitHub Actions は push / pull request で同じ検証とリリースビルドを macOS / Linux で実行します。CI の設定とリモートでの実行結果は分けて確認してください。

## 配布とドキュメント

公開・配布ライセンスは未設定で、`Cargo.toml` は `publish = false` にしています。

バージョンは `Cargo.toml` の `package.version` を正本とし、対応する `vX.Y.Z` タグと GitHub Releases で管理します。初回は `v0.1.0` です。通常の修正は patch、機能追加は minor、互換性を壊す変更は major を更新します。`Cargo.lock` と `giclean --version` も同じバージョンに揃えます。`--version` はビルド時に `Cargo.toml` から埋め込まれます。

リリース手順:

1. `Cargo.toml` のバージョンを更新し、`cargo check` で `Cargo.lock` を同期します。初回は既存の `0.1.0` を使います。
2. `make check` と `make build` を実行し、変更を `main` に反映します。
3. リリース対象の `main` のコミットに注釈付きタグを作り、そのタグだけを push します。初回の例:

   ```sh
   git tag -a v0.1.0 -m "giclean v0.1.0"
   git push origin v0.1.0
   ```

4. GitHub Actions の `Release` が成功したことと、[GitHub Releases](https://github.com/kosnu/giclean/releases) の公開内容を確認します。

`Release` はタグと `Cargo.toml` の一致を確認し、macOS / Linux の x86_64 / aarch64 それぞれで `make check` と `make build` を実行します。すべて成功した場合に、バージョン・ターゲット名付きの `.tar.gz`、`SHA256SUMS`、自動生成したリリースノートを公開します。Linux 版は Ubuntu 24.04 上でビルドするため、配布先には glibc 2.39 以降が必要です。

`0.2.0-rc.1` のようなバージョンと `v0.2.0-rc.1` タグは GitHub の prerelease として公開します。一度公開したタグは移動・再利用せず、修正は新しいバージョンでリリースします。crates.io への公開は行いません。

利用開始に必要な情報は [README](README.md)、コマンドの仕様と制約は [br の仕様](docs/commands/br.md)、設計判断や SSH 接続の説明は [設計](docs/design.md)に記載します。
