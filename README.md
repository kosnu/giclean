# giclean

Git のお掃除コマンドです。

## インストール

macOS / Linux 対応。実行には Git 2.47 以降が必要です。

[GitHub Releases](https://github.com/kosnu/giclean/releases) から OS・CPU に合う `.tar.gz` をダウンロードし、展開した `giclean` を PATH の通ったディレクトリに配置します。macOS / Linux ともに x86_64 / aarch64（ARM64）向けを配布します。Linux 版は glibc 2.39 以降が必要です。

同じリリースの `SHA256SUMS` でファイルの整合性を確認できます。バージョンは `giclean --version` で確認します。

ソースからインストールする場合は Rust 1.89.0 も必要です。

```sh
cargo install --path . --locked
```

## 更新

配布バイナリを使っている場合は、[GitHub Releases](https://github.com/kosnu/giclean/releases) から更新したいリリースの OS・CPU に合う `.tar.gz` をダウンロードします。同じリリースの `SHA256SUMS` で整合性を確認し、展開した `giclean` で配置済みのバイナリを置き換えます。

ソースからインストールした場合は、ソースを更新したうえで、そのディレクトリで再インストールします。

```sh
cargo install --path . --locked
```

`--path` を指定すると毎回ビルド・インストールするため、通常は `--force` は不要です。

更新後はバージョンを確認します。

```sh
giclean --version
```

## 使い方

整理したいリポジトリ内で実行します。

```sh
giclean br --dry-run  # 削除候補を確認
giclean br            # 確認なしで削除
giclean --help
```

`origin` の最新の既定ブランチにマージ済みのローカルブランチを削除します。
通常のマージに加え、squash などで変更した全パスの内容・権限が、既定ブランチの現在または過去の同一コミットで完全一致するブランチも対象です。過去の比較は第一親の履歴を使い、取り込み後に同じファイルが更新・取り消しされた場合も対象にします。未取り込みの追加変更がある場合は保持します。
`main` / `master` / `production`、既定ブランチ、使用中のブランチは保持します。
自動 fetch は行いません。最新コミットが未取得の場合は `git fetch origin` 後に再実行してください。

## ドキュメント

- [br の仕様・出力・制約](docs/commands/br.md)
- [設計・SSH 接続・背景資料](docs/design.md)
- [開発・検証手順](CONTRIBUTING.md)
