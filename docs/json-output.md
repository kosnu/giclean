# JSON 出力（schema version 1）

`br` と `wt` で `--format json` を指定できます。省略時と `--format text` は従来のテキスト出力です。追加の実行時依存はありません。

```sh
giclean br --dry-run --format json
giclean br --format json
giclean wt -d 7 --dry-run --format json
```

`--explain` と併用しても JSON には説明行を追加しません。判定理由は `decisions` で確認してください。

## 出力と終了状態

有効な cleanup コマンドでは、stdout に UTF-8 の JSON object を1個と末尾の改行だけ出力します。候補がない場合も同じ形式です。進捗・Git の診断・致命的エラーの説明は stderr に出ます。実行中に失敗しても、それまで収集した判定と実行結果を含む JSON を出力します。stdout 自体の書き込み失敗、プロセスの強制終了などでは完全な出力は保証できません。

- 終了コード `0`: 完了。削除対象なし・dry-run も含みます。
- 終了コード `1`: 照会・再検査の失敗、削除失敗、出力失敗など。
- 終了コード `2`: 不正な引数。stdout は空で、stderr に説明を出します。

`--format` は `br` / `wt` のオプションで、値は `text` / `json` のみです。重複指定・値の省略はエラーです。`--format json` と help の組み合わせも引数エラーです。help は `giclean br --help` / `giclean wt --help` で確認してください。グローバルな `--help` / `--version` は従来通りテキストです。

[機械可読スキーマ](json-schema-v1.json) も参照できます。

## スキーマ

トップレベルのフィールドは次の通りです。`null` は値がないことを表します。

| フィールド | 型 | 意味 |
| --- | --- | --- |
| `schema_version` | integer | `1` |
| `command` | string | `br` または `wt` |
| `dry_run` | boolean | 削除せず候補を確認したか |
| `success` | boolean | cleanup 処理が成功したか。stdout 書き込み失敗は反映できない場合があります |
| `decisions` | array | 各対象について最後に観測した判定 |
| `executions` | array | 実行結果。判定とは別に扱います |
| `error` | object / null | 処理全体を中断した失敗。`code: "cleanup_failed"` と `message: string` |

`decisions[]`:

- `target`: 下記の対象情報。
- `eligibility`: `deletable` / `protected` / `unknown`。`unknown` は削除許可ではありません。
- `reason_code`: 判定の安定した機械可読コード。
- `inspected_at`: `{ "unix_seconds": integer, "nanoseconds": integer }`。観測時刻であり、現在も有効である保証ではありません。Unix epoch より前の時刻は `null`。
- `evidence`: `merged_at`, `latest_activity`, `cutoff`。各値は `inspected_at` と同じ時刻形式または `null`。worktree のマージ日時・最終活動日時・待機期限の比較に使用した観測値です。未照会の場合は `null`。
- `detail`: 補足診断 string / `null`。機械判定には使用しないでください。

`executions[]`:

- `target`: 下記の対象情報。
- `outcome`: `candidate` / `deleted` / `failed` / `skipped` / `not_attempted`。
- `reason_code`: `failed` の場合 `deletion_failed`、それ以外は `null`。
- `detail`: 削除失敗の診断 string / `null`。

対象情報:

- branch: `{ "kind": "branch", "name": string, "oid": string }`
- worktree: `{ "kind": "worktree", "path": string, "branch": string | null, "oid": string }`

OID は Git の object format に従います（長さを SHA-1 に固定しないでください）。worktree の path は絶対パスです。文字列内の引用符・バックスラッシュ・制御文字は JSON escape されます。Git の一覧を UTF-8 として解釈できない場合は保守的に失敗し、非 UTF-8 名を勝手に置換して削除しません。

## 判定と実行結果の読み方

`deletable` は観測時点の適格性であり、削除成功ではありません。削除成功は `executions[].outcome == "deleted"` で確認します。dry-run は `candidate` を報告し、削除しません。再検査で状態が変われば最後の判定は更新されます。途中の fatal error により全対象の判定や実行結果が揃わない場合があります。配列に対象がないことを削除成功と解釈しないでください。

個別削除に失敗して他の削除を継続した場合、`success` は `false` になり、成功・失敗の両方を `executions` に残します。この場合、処理全体の中断がなければ `error` は `null` です。一括参照削除後に設定の後片付けが失敗した場合も `failed` になり得ます。`failed` は「変更がなかった」を意味しません。

## reason code

削除可能: `merged`, `squash_included`, `inactive_merged`。

保護: `protected_branch`, `branch_in_use`, `symbolic_branch`, `default_branch`, `not_merged`, `main_worktree`, `unborn_head`, `detached_head`, `bare_worktree`, `locked_worktree`, `prunable_worktree`, `current_worktree`, `git_operation`, `index_flags`, `local_changes`, `waiting_period`, `recent_activity`。

不明: `state_changed`, `target_missing`, `path_unavailable`, `no_verified_merge`, `inspection_failed`, `not_inspected`, `agent_unknown`。

エージェントの live 参照による保護は `agent_active` です。初期の Codex detector は live の確定を
行わず、対象 home が存在する場合は `agent_unknown`、detail は `codex: incomplete_coverage` などの
固定分類を返します。対象範囲と保護が広がる制約は [Codex 保護](agents/codex.md)を参照してください。

実行失敗: `deletion_failed`。処理全体の失敗: `cleanup_failed`。

## 互換性方針

`schema_version` が同じ間は既存の必須フィールドの削除・型変更・意味変更は行いません。任意フィールド、reason code、outcome、対象種別は追加される可能性があります。利用側は未知のフィールドを無視し、未知のコードや種類は保守的に扱ってください。破壊的変更時は `schema_version` を上げます。

キーの順序、配列の順序、空白、`detail` / `message` の文言、時刻の正確な値は互換性保証に含みません。判定・実行結果は `target` で対応付け、配列の添字で結合しないでください。reason code の代わりに診断文を解析することも避けてください。
