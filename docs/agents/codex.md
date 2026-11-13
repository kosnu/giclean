# Codex による worktree 保護

`wt` / `batch wt` は `giclean::agents::codex::CodexDetector` による追加の削除拒否を行います。
対象環境は Linux / macOS のローカルファイルシステムです。Codex の導入・起動、
セッション履歴の読み取り・削除、Codex の lock の変更は行いません。

## 初期実装の範囲

各観測で環境変数を読み直し、明示された `CODEX_HOME` を優先します。
未指定の場合だけ `HOME/.codex` を確認します。設定値は絶対パスを必要とします。
調べるのは home 自体のファイルシステムメタデータだけです。配下は列挙せず、
rollout、state DB、設定、認証情報、会話本文、タイトル、指示の内容は開きません。
Codex コマンドや App Server を起動せず、プロセス探索やネットワーク接続もしません。

| 観測 | 結果 | `wt` の扱い |
| --- | --- | --- |
| 有効な `HOME` ディレクトリの下に `.codex` がない | 対象の home 探索範囲で `NoReference` | 既存の Git 条件の検査を続ける |
| 対象 Codex home がディレクトリとして存在する（空でも同じ） | `IncompleteCoverage` | 全 worktree を `agent_unknown` として保持 |
| 明示 home がない、HOME が不明・存在しない、読めない、相対・空パス、通常ファイル、Codex home が symlink（リンク切れを含む） | 固定の失敗分類 | 全 worktree を `agent_unknown` として保持 |

対象 home の存在だけでは、現在の利用状況や対象 worktree を特定できません。
そのため、初期実装は live の `Active` と断定せず、不明として広く保護します。
実行中・loaded-idle・承認待ち・入力待ちを区別して解除することもありません。
古い履歴だけ、archive 済み、別 worktree を参照した履歴だけ、空ディレクトリ、
未知版・圧縮・破損・読取不能のデータがある場合も、home がある限り保持します。
これらの形式を解析・対応しているという意味ではありません。

この選択では終了済みの Codex データしかなくても `wt` の削除ができなくなります。
対応範囲に home がない Codex 非導入環境では従来の Git 判定を維持します。
保護を解除する CLI オプションは用意しません。home を隠したり削除したりして
未使用を証明する設計ではありません。

観測範囲は選択された home 一つです。別環境で指定された別 home、独自設定で移動した
home、別ユーザー、別ホスト、リモート環境、他のエージェントまでは網羅しません。
既定 home の不在は、これらを含む全 Codex プロセスの不在・未使用証明ではありません。
確実に保持したい対象には `git worktree lock` を使えます。

## Git 判定と出力

Git の worktree 一覧を対象の正本とし、既存の主/current worktree・ブランチ・ロック等の
保持理由を先に適用します。残る対象で agent 判定が不明なら、PR 照会前に保持します。
agent 側に拒否がない場合も、dirty/ignored/操作途中/PR マージ/Git 包含/待機期間などの
既存の条件をすべて通す必要があります。`br` にはこの検出を追加しません。

初回検査と、Git の安全性・活動時刻を再確認した後の削除／dry-run 候補出力直前に
同じ detector で再観測します。新たな home・検出不能が見つかれば保持します。
再観測後の開始・resume・home 移動との競合は残り、原子的な安全性は保証しません。

`--explain` と JSON は同じ判定を使い、`agent_unknown` と
`codex: incomplete_coverage` などの固定 detail を表示します。Codex home のパス、
セッション識別子、内容、生の I/O エラーは出力しません。保持だけなら終了コードは `0` です。
初回から保持したものは `not_attempted`、直前再検査による保持は `skipped` です。
`agent_active` は共通 API が live 参照を返す場合の保護理由として接続していますが、
この初期 detector は live 参照の確定を行いません。

## 根拠と検証

設計の出発点は [#24 の調査結果](https://github.com/kosnu/giclean/issues/24#issuecomment-6821707702)です。
公開ソース `rust-v0.167.2` の以下を確認しました。

- [home の解決](https://github.com/openai/codex/blob/rust-v0.167.2/codex-rs/core/src/config/mod.rs#L5240-L5248): `CODEX_HOME` を優先し、未指定なら `~/.codex`。
- [loaded 一覧と status](https://github.com/openai/codex/blob/rust-v0.167.2/codex-rs/app-server-protocol/src/protocol/v2/thread.rs#L1691-L1733): サーバーのメモリ内の thread 一覧と active/idle/待ち状態。全 CLI/サーバーの未使用証明ではないと判断しています。
- [SessionMeta](https://github.com/openai/codex/blob/rust-v0.167.2/codex-rs/protocol/src/protocol.rs#L3418-L3447): 作成時 cwd と任意の workspace roots。現在の非稼働判定には使いません。

既存 App Server の完全な live 観測は未実装で、Codex 版ごとの実機保証もありません。
履歴や mtime、空の loaded 一覧から削除許可を出さず、将来の検出器の拡張余地を残しています。
テストは隔離した HOME/CODEX_HOME と合成データだけを使い、実セッションを読みません。
一時 Git リポジトリで、home の有無・異常、複数 worktree、各状態・未知/圧縮/途中書込みの
データがあっても保持すること、照会中・最終 Git 再検査中の home 出現、情報源消失、
dry-run/実削除の再検査、出力とメタデータの維持、`br` の動作維持を検証します。
