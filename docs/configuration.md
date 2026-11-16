# 設定ファイル

`giclean --config PATH <command> [options]` で明示したファイルだけを読みます。
自動探索・リポジトリ内の設定の暗黙読込はしません。指定しなければ従来と同じ動作です。
ファイルがない、読み取れない、不正な設定がある場合は、Git の探索や削除を始めず終了コード 2 で失敗します。

```text
# ~/.config/giclean/defaults.conf の例
path = /Users/me/dev
path = /Users/me/other-projects
max-depth = 3
exclude = archived
exclude = vendor
wt-days = 5
```

```sh
giclean --config ~/.config/giclean/defaults.conf repos
giclean --config ~/.config/giclean/defaults.conf batch wt --dry-run --explain
giclean --config ~/.config/giclean/defaults.conf wt --dry-run --explain
# CLI の明示値で設定を上書き
giclean --config ~/.config/giclean/defaults.conf batch wt --hours 24 --dry-run .
```

## 形式

UTF-8 の `キー = 値` を一行ずつ記載します。空行と、前後の空白を除いた行の先頭が `#` のコメントを無視します。
キーと値の前後の空白は取り除き、それ以外の値はそのまま使用します。
引用符・エスケープ・行末コメント・環境変数・`~` の展開はありません。空白を含むパスにも引用符を付けません。
不明なキー、空の値、制御文字、単一項目の重複、不正な値はエラーです。TOML や shell の構文ではありません。

| キー | 対象 | 値 |
| --- | --- | --- |
| `path` | `repos`, `batch br`, `batch wt` | 探索ルート。複数行可。相対パスは設定ファイルの実体のあるディレクトリ基準 |
| `max-depth` | 同上 | 0 以上の整数 |
| `exclude` | 同上 | 除外するディレクトリの正確な basename。複数行可。glob やパスは不可 |
| `wt-days` / `wt-hours` | `wt`, `batch wt` | 待機時間。正の整数でいずれか一つだけ指定 |

`wt` の待機時間は既存どおりマージ後またはそれより新しい活動後の時間です。
設定ファイルは一度だけ読み込み、batch の各プロセスには解決済みの CLI 引数を渡します。

## 優先順位

項目ごとに **明示的な CLI 引数 > 設定ファイル > 既存の既定値** です。
CLI に組み込み既定値と同じ値を指定した場合も、その明示値を優先します。

- CLI に探索パスが一つでもあれば、設定の `path` リスト全体を置き換えます。CLI の相対パスは実行ディレクトリ基準です。
- CLI の `--exclude` があれば、設定の除外リスト全体を置き換えます（追加ではありません）。
- CLI の `--days` / `--hours`（`-d` / `-h`）のどちらでも設定の待機時間を置き換えます。
- 未設定なら探索は `.`、深さ 3、追加の除外なし。`wt` の待機時間には既定値を追加しません。
- 設定の不正値は CLI で上書きしていてもエラーにします。CLI の重複・不正値も従来どおりエラーです。
- 全キーを検証しますが、対象外のコマンドには適用しません。help はコマンドを実行しません。

## 安全性

設定は削除の許可ではありません。`--execute`、`--dry-run`、`--include-detached`、`--allow-ignored` は保存できません。
特に batch の実行モード、`wt prune` の待機時間と実行モードは引き続き毎回明示してください。
`br` / `wt` の従来の確認なし削除は変わりません。候補確認には必ず `--dry-run --explain` を指定してください。
`wt` は設定に待機時間があれば CLI で時間を指定せず実行できるため、設定内容を確認して使ってください。
保護対象、no-fetch、保守的な削除判定、dry-run、削除直前の再検査は変更しません。
