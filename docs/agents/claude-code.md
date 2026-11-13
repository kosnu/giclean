# Claude Code の公開情報調査

調査日: 2026-11-13。[#26](https://github.com/kosnu/giclean/issues/26) の公開資料調査です。
Claude Code の契約・アカウント作成・実行・実セッションの読み取りは行っていません。
参照した公式ドキュメントは更新される資料であり、特定のインストール版の実機検証ではありません。

## 結論

**Worktree との関連付けと、参照ありを理由に保護する detector は設計可能と判断します。
現在の全セッションの非稼働を証明する detector の対応保証は、今回の資料調査からはできません。**
これは以下の公式仕様からの設計上の推論です。今回 Claude detector や hook は実装・登録せず、
将来 [汎用 API](../design.md#エージェント検出の境界) に追加する候補とします。

## 確認できた事項と限界

| 公開情報で確認できた事項 | giclean で利用する場合の判断・限界 |
| --- | --- |
| SDK の `list_sessions()` は過去セッションを列挙し、`cwd` と更新時刻等を返す。`include_worktrees` で関連 Worktree の履歴も含められる | 関連付けの材料。稼働一覧ではないため履歴だけなら `Historical`。ページング/範囲の不足は `Unknown`。[SDK 公式資料](https://code.claude.com/docs/en/agent-sdk/python#list-sessions) |
| 同 API の結果には `summary`、`first_prompt` 等も含まれる | 「メタデータ API」でも会話由来の内容が混ざる。将来の実装は `cwd` 等の必要項目だけを扱う方式を検討し、応答全体を収集/ログ出力しない。[返却型](https://code.claude.com/docs/en/agent-sdk/python#return-type-sdksessioninfo) |
| hook 共通入力には `session_id` と実行時 `cwd` がある。`SessionStart` は開始/再開、`SessionEnd` は終了時のイベント。`Stop` は応答完了時のイベント | 開始/再開の正の証拠を取る候補。`Stop` をセッション終了扱いしない。hook 未導入・無効化・失敗・取りこぼしを非稼働の根拠にしない。[hooks 公式資料](https://code.claude.com/docs/en/hooks#common-input-fields)、[開始](https://code.claude.com/docs/en/hooks#sessionstart)、[終了](https://code.claude.com/docs/en/hooks#sessionend)、[Stop](https://code.claude.com/docs/en/hooks#stop) |
| hook の `cwd` は Worktree/作業先の変更に追随する一方、`CLAUDE_PROJECT_DIR` は開始元に残る | 開始元の環境変数だけで Worktree を識別できない。複数作業先を落とさない設計が必要。[パスの扱い](https://code.claude.com/docs/en/hooks#reference-scripts-by-path) |
| SDK は会話履歴を保持して再開する仕組みを持つ | 履歴の存在・古さを現在の非稼働に読み替えない。本文読み取り API を検出に使わない。[セッションの公式説明](https://code.claude.com/docs/en/agent-sdk/sessions) |

## 未確認事項

- 全 CLI・Desktop・SDK・別 home・リモートを網羅した、同一ホストの現在の参照を取得できる公開インターフェースと、その完全性。
- hook が利用者の全セッションで有効だったことの検証方法、異常終了・強制終了・再開競合を含むイベント配送の完全性。
- 各導入版の互換性、複数 cwd / subagent / Worktree 移動時の観測範囲、symlink や消失パスの照合、必要項目だけを安全に取得する実装方式。
- 実環境での検出精度・性能。対応版や全版互換を宣言できる試験はしていません。

上記は「機能が存在しない」という断定ではなく、本調査で安全な完全性を確認できていない事項です。
将来実装する場合も、未対応版・解析/読取失敗・範囲不明は `Unknown` / `Err`、履歴のみは
`Historical` として保持します。自動的な stale 判定やロック掃除は行いません。
偽陽性によって不要な Worktree を保持する不便を許容し、作業中 Worktree を削除する偽陰性を避けます。

将来の検証には公開仕様から作った合成 fixture と一時 Git リポジトリを使い、
active/idle/承認・入力待ち/履歴のみ/不明/エラー、複数作業先、初回観測後の状態変化、
dry-run と削除直前の再検査を含めます。Claude Code の実機利用は本 Issue の完了条件に含みません。

Codex 側の安全方針の根拠は [#24 の調査コメント](https://github.com/kosnu/giclean/issues/24#issuecomment-6821707702)
です。Codex 固有の検出と `wt` 接続は #25 の範囲で、今回の調査には含めていません。
