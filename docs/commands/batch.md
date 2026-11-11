# 複数リポジトリの一括クリーンアップ

```sh
giclean batch br --dry-run --explain ~/dev
giclean batch br --execute --max-depth 4 --exclude node_modules ~/dev
giclean batch wt --days 3 --dry-run ~/dev ~/work
giclean batch wt --hours 72 --execute ~/dev
```

`batch br` / `batch wt` は `repos` と同じ探索・重複排除を使い、発見した各リポジトリのディレクトリで既存の `br` / `wt` を起動します。通常の単一リポジトリコマンドの挙動は変えません。一括処理では **`--dry-run` または `--execute` のいずれかが必須**です。`--execute` は確認なしで削除します。

- パス省略時は現在のディレクトリ。ルートが深度0、既定の最大深度は3です。
- `--exclude NAME` は正確に一致するディレクトリ名を除外し、複数回指定できます。globは展開しません。
- `--` 以降はすべて探索パスとして扱います。
- シンボリックリンクは追跡しません。linked worktree は common Git directory で重複排除し、探索範囲内の主 worktree を優先します。
- bare repository はスキップします。
- 子プロセス起動直前にもリポジトリの同一性を再確認します。既存の保護条件、マージ判定、削除直前の再検査を省略しません。自動fetchは行いません。
- `wt` では従来どおりGitHubのoriginと認証済み`gh`が必要です。

探索・リポジトリ処理でエラーがあっても他の有効なリポジトリを処理します。**トランザクションではありません。途中のエラー時にも他のリポジトリで削除が実行される場合があります。** 結果はリポジトリパスの見出しごとに出力し、最後に成功・失敗・スキップ数を表示します。探索エラーはstderrに別途表示されます。終了コードは全処理成功で0、探索またはクリーンアップ失敗で1、引数エラーで2です。対象0件は成功です。

一括出力は現在textのみです。`--format json`は削除前に引数エラーになります。JSONが必要な場合は各リポジトリで既存の`br` / `wt --format json`を実行してください。

環境変数による別リポジトリへの誤操作を防ぐため、`GIT_DIR`、`GIT_WORK_TREE`、`GIT_COMMON_DIR`、`GIT_INDEX_FILE`、`GIT_OBJECT_DIRECTORY`、`GIT_ALTERNATE_OBJECT_DIRECTORIES`、`GIT_NAMESPACE`、`GIT_SHALLOW_FILE`、`GIT_CONFIG`、`GIT_CONFIG_COUNT`、`GIT_CONFIG_PARAMETERS`が設定されている場合は、探索・削除前に中止します。通常のグローバル設定、credential helper、SSH設定は引き継ぎます。
