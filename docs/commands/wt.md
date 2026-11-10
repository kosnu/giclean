# wt

GitHub の `origin` の既定ブランチに取り込まれ、マージ後に指定時間以上アクティビティのない linked worktree を削除します。worktree に対応するブランチは保持します。

## 使い方

```sh
giclean wt -d 3 --dry-run  # 候補を確認
giclean wt -d 3            # 3日（72時間）を指定
giclean wt -h 12           # 12時間を指定
giclean wt --help
```

`-d` / `--days` または `-h` / `--hours` のどちらか一つに正の整数を指定します。両方の指定・重複・ゼロ・小数はエラーです。`wt` の `-h` は時間指定なので、ヘルプには `--help` を使います。通常実行は確認なしで削除し、`--dry-run` は同じ条件で候補を表示します。

Git 2.47 以降に加え、認証済みの [GitHub CLI (`gh`)](https://cli.github.com/) が必要です。`origin` は github.com の HTTPS または SSH URL（`git@github.com:owner/repo.git` / `ssh://git@github.com/owner/repo.git`）に対応します。GitHub Enterprise・SSH ホストの別名・他サービスは対象外です。`GH_REPO` や gh の既定リポジトリ設定による照会先の変更は行いません。

## 時間の起点

次のうち新しい時刻を起点にして、指定期間以上経過している場合だけ対象にします。

- 対応する PR の実際の `merged_at`。Git のコミット日時を代用しません。
- worktree 内のファイル・ディレクトリ・シンボリックリンクの最終更新時刻（mtime）。ファイルの追加・削除・同じ内容での保存なども更新として扱います。
- その worktree 固有の Git 管理ディレクトリ内の最終更新時刻。HEAD、index、HEAD reflog などを含み、checkout・reset・commit などによる更新を検出します。

例えば、月曜にマージし、水曜に最後の操作をした worktree は、`-d 3` なら水曜の操作から72時間経過して初めて対象になります。マージ前の最終操作が古くても、マージ直後には削除しません。未来の更新時刻がある場合も保持します。

ファイルの読み取り、端末の利用、保存していないエディタ上の編集、他プロセスが開いていることは検出できません。mtime を保持するコピー・復元や時計のずれも検出の限界です。長期間保持したい worktree は `git worktree lock` で保護できます。

検査時は `GIT_OPTIONAL_LOCKS=0` で index の任意更新を抑止します。dry-run や候補検査自体で待機時間をリセットしません。実行をまたぐ独自の時刻記録は作りません。

## マージと取り込みの確認

1. `git worktree list --porcelain -z` で一覧を取得し、保護対象を除きます。
2. `git ls-remote --symref --exit-code origin HEAD` で最新の既定ブランチとコミットを確認します。コミットがローカルに未取得なら `git fetch origin` を案内して停止します。自動 fetch は行いません。
3. `gh api` で worktree の先端コミットに関連する PR を全ページ照会します。[GitHub のコミット関連 PR API](https://docs.github.com/en/rest/commits/commits#list-pull-requests-associated-with-a-commit)から、マージ日時・先端 OID・ブランチ名・マージ先ブランチ・マージ先リポジトリ・マージ結果の OID を読み取ります。
4. PR の先端 OID・ブランチ名が worktree と一致し、マージ先が `origin` の既定ブランチで、マージ結果のコミットが最新の既定ブランチの祖先であることを検査します。
5. worktree の先端がそのマージ結果の祖先なら取り込み済みと判定します。祖先でない場合も、既存の `br` と同じ全変更パスの内容・権限の完全一致比較で確認します。PR の状態だけやパッチの類似性では判定しません。
6. 条件を満たす PR が複数あれば、最も新しいマージ日時を使います。日時不明・対応する PR なし・別ブランチ向け・未取り込みの追加変更があるものは保持します。

通常のマージ・squash・fast-forward に対応します。GitHub の PR を介さない取り込みは、マージ日時を確認できないため保持します。マージ後にローカルブランチを改名した場合や先端が変わった場合も、PR 情報と一致しなければ保持します。

## 保護と削除

次の worktree は削除しません。

- 主 worktree、現在コマンドを実行している worktree（サブディレクトリからの実行も含む）。
- `main` / `master` / `production` / `origin` の既定ブランチ、symbolic なローカル参照。
- ロック済み、detached HEAD、prunable なもの。
- 未コミット変更、ステージ済み変更、未追跡ファイル、ignored ファイルがあるもの。
- index に `assume-unchanged` / `skip-worktree` のエントリがあるもの。`git status` が変更を表示しない可能性があるため、実際に変更がなくても保持します。index のフラグは変更しません。
- merge・rebase・cherry-pick・revert・bisect などの操作状態、Git の HEAD/index ロックが残るもの。

全候補の初期検査を終えてから削除を開始します。初期検査と各操作直前の両方で、`git ls-files --cached -v -z` による index フラグの検査を行います。照会失敗・不正な応答は削除中止とします。各操作直前に一覧・先端・ブランチ・ロック・ローカル変更・更新時刻も再検査し、変わっていれば保持します。削除には強制オプションなしの `git worktree remove -- <path>` を使います。[Git の worktree remove](https://git-scm.com/docs/git-worktree#Documentation/git-worktree.txt-remove)が作業ディレクトリと登録情報を削除します。

再確認と削除は原子的ではありません。最終確認後の同時操作を完全には防げないため、別プロセスからの書き込みと同時に実行しないでください。特に Git 自体は ignored ファイルの削除を拒否しないため、独自の直前検査で保護しています。

## 出力と失敗

CLI のメッセージは英語です。

| 標準出力 | 意味 |
| --- | --- |
| `candidate: <path>` | dry-run の削除候補 |
| `<path>: deleted` | 削除成功 |
| `<path>: failed (<reason>)` | 削除失敗 |

対象がなければ標準出力は空です。終了コードは正常・対象なしが `0`、照会・検査・削除失敗が `1`、引数不正が `2` です。進捗と診断は標準エラーに表示します。

GitHub API・認証・Git の照会失敗、不正な応答、時刻取得失敗は削除開始前に停止します。削除中の再確認に失敗した場合は以降を停止し、それまでの削除結果は表示します。個別の削除失敗は報告して残りを処理し、終了コード `1` を返します。成功済みの削除は取り消しません。

## 判定理由の確認

`--explain` を指定すると、削除候補だけでなく保持した対象の理由も標準出力へ表示します。

```sh
giclean wt -d 3 --dry-run --explain  # 削除せずに判定理由を確認
giclean wt -d 3 --explain            # 理由を表示し、削除対象は削除
```

`--explain` 自体は dry-run ではありません。削除を避ける場合は必ず `--dry-run` を併用してください。
説明は同じ判定結果から生成し、保護条件や削除直前の再検査を変更しません。未確認の条件は推測せず、先に該当した保持理由を表示します。
指定しない場合の出力は従来どおりです。

`explain:` 行の `eligible` は削除条件を満たした判定、`kept` は保持の判定です。続く `execution:` 行で dry-run の候補・未実行・再検査によるスキップ・削除済み・失敗を区別します。検査で得た理由と、削除が実際に成功したかは別々に表示します。

検証できた PR マージ時刻、最後のローカル活動時刻、待機期限を Unix epoch からの秒数で表示し、検査時点での経過秒数も併記します。安全上の理由で先に保持した対象では、調べていない時刻やマージ状況は表示しません。
