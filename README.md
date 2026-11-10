# giclean

Git のお掃除コマンドです。

## インストール

macOS / Linux 対応。実行には Git 2.47 以降が必要です。

[GitHub Releases](https://github.com/kosnu/giclean/releases) から OS・CPU に合う `.tar.gz` をダウンロードし、展開した `giclean` を PATH の通ったディレクトリに配置します。macOS / Linux ともに x86_64 / aarch64（ARM64）向けを配布します。Linux 版は glibc 2.39 以降が必要です。

同じリリースの `SHA256SUMS` でファイルの整合性を確認できます。バージョンは `giclean --version` で確認します。

ソースからインストールする場合は Rust 1.89.0 も必要です。

```sh
make install
```

## 更新

配布バイナリを使っている場合は、[GitHub Releases](https://github.com/kosnu/giclean/releases) から更新したいリリースの OS・CPU に合う `.tar.gz` をダウンロードします。同じリリースの `SHA256SUMS` で整合性を確認し、展開した `giclean` で配置済みのバイナリを置き換えます。

ソースからインストールした場合は、ソースを更新したうえで、そのディレクトリで再インストールします。

```sh
make install
```

`make install` は `cargo install --path . --locked` を実行します。毎回ビルド・インストールするため、通常は `--force` は不要です。

更新後はバージョンを確認します。

```sh
giclean --version
```

## 使い方

整理したいリポジトリ内で実行します。

```sh
giclean br --dry-run  # 削除候補を確認
giclean br            # 確認なしで削除
giclean wt -d 3 --dry-run  # マージ後3日以上アクティビティのない worktree を確認
giclean wt -d 3            # 対象の worktree を削除
giclean wt -h 12           # 待機時間を12時間に指定
giclean --help
```

`origin` の最新の既定ブランチにマージ済みのローカルブランチを削除します。
通常のマージに加え、squash などで変更した全パスの内容・権限が、既定ブランチの現在または過去の同一コミットで完全一致するブランチも対象です。過去の比較は第一親の履歴を使い、取り込み後に同じファイルが更新・取り消しされた場合も対象にします。未取り込みの追加変更がある場合は保持します。
`main` / `master` / `production`、既定ブランチ、使用中のブランチは保持します。
自動 fetch は行いません。最新コミットが未取得の場合は `git fetch origin` 後に再実行してください。

`wt` は、GitHub の PR マージ日時と worktree の最終更新日時のうち、新しい方から指定時間が経過した linked worktree を削除します。マージ後にファイルの更新や Git 操作があれば、その時点から数え直します。通常のマージ・squash・fast-forward に対応し、取り込み済みかどうかも Git で検査します。

`wt` には、github.com の `origin` と認証済みの [GitHub CLI (`gh`)](https://cli.github.com/) が必要です。マージ日時が不明なもの、現在の worktree、主 worktree、ロック済み、未コミット変更・未追跡ファイル・ignored ファイルがあるものは保持します。ブランチ自体は削除しません。読み取りだけの利用はアクティビティとして検出できません。詳しい判定条件は [wt の仕様](docs/commands/wt.md)を参照してください。

## ドキュメント

- [br の仕様・出力・制約](docs/commands/br.md)
- [wt の仕様・出力・制約](docs/commands/wt.md)
- [設計・SSH 接続・背景資料](docs/design.md)
- [開発・検証手順](CONTRIBUTING.md)

## JSON 出力

`--format json` で判定理由と実行結果を機械可読形式で出力できます。`--dry-run` と併用できます。仕様と互換性方針は [JSON 出力](docs/json-output.md) を参照してください。
