# giclean

Git のお掃除コマンドです。

## インストール

Rust 1.89.0 / Git 2.47 以降が必要です。macOS / Linux 対応。

```sh
cargo install --path . --locked
```

## 使い方

整理したいリポジトリ内で実行します。

```sh
giclean br --dry-run  # 削除候補を確認
giclean br            # 確認なしで削除
giclean --help
```

`origin` の最新の既定ブランチにマージ済みのローカルブランチを削除します。
`main` / `master` / `production`、既定ブランチ、使用中のブランチは保持します。squash マージは判定対象外です。
自動 fetch は行いません。最新コミットが未取得の場合は `git fetch origin` 後に再実行してください。

## ドキュメント

- [br の仕様・出力・制約](docs/commands/br.md)
- [設計・SSH 接続・背景資料](docs/design.md)
- [開発・検証手順](CONTRIBUTING.md)
