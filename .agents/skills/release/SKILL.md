---
name: release
description: giclean のリリース準備・公開・公開確認を行う。バージョン更新、リリース用 PR、タグ、GitHub Releases を扱う依頼に使う。
---

# giclean のリリース

最初に [AGENTS.md](../../../AGENTS.md)、[配布手順](../../../CONTRIBUTING.md#配布とドキュメント)、[Release ワークフロー](../../../.github/workflows/release.yml) を読む。準備のみ・説明のみの依頼で公開しない。公開まで依頼されたら、以下の順で公開確認まで進める。

## 入力と実行規則

コマンドはリポジトリルートから一つずつ実行する。大文字のトークンは下表の確定値に置き換える。シェル変数の引き継ぎには依存しない。

| 入力 | 決め方 |
| --- | --- |
| `VERSION` | `v` を除いた次のバージョン。判断基準は次節 |
| `BASE_TAG` | 今回の比較元となる公開済みタグ |
| `BRANCH` | `release-vVERSION`。同じバージョンの準備があれば再利用 |
| `PREPARE_COMMIT` | 準備コミットの完全な SHA |
| `PR_NUMBER` | 作成または再利用するリリース用 PR 番号 |
| `RELEASE_COMMIT` | マージ後の `origin/main` の完全な SHA |
| `CI_RUN_ID` / `RUN_ID` | 対象コミットの CI / 対象タグの Release の実行 ID |

各工程の成功条件を満たしてから次へ進む。再開時は同じ入力に対する成功条件を再確認し、完了済みの変更操作を省略する。コマンド失敗・値の不一致・候補の重複は停止条件。API の通信失敗を「存在しない」と解釈しない。変更操作は許可された範囲だけ行い、同じ範囲の承認を繰り返し求めない。

## 1. 対象とバージョンの確定

ここでは PR・CI を照会しない。

| コマンド | 成功条件・記録する値 |
| --- | --- |
| `git status --porcelain` | 空。再開時の変更は `Cargo.toml`・`Cargo.lock` の予定したバージョン更新だけなら継続可 |
| `git remote get-url origin` | `git@github.com:kosnu/giclean.git` または `https://github.com/kosnu/giclean.git`。それ以外は対象を確認するまで停止 |
| `git fetch origin main --tags` | 終了コード `0` |
| `git rev-parse origin/main` | 対象 main の完全な SHA を記録 |
| `gh release list --repo kosnu/giclean --limit 20 --json tagName,isDraft,isPrerelease,publishedAt` | 公開済みリリースから `BASE_TAG` を選択。比較元が表示されなければ件数を増やす |
| `git log BASE_TAG..origin/main --oneline` | 含めるコミットを特定 |
| `git diff BASE_TAG..origin/main --stat` | 変更範囲を確認。バージョン更新以外の未公開差分がなければ、既存リリースの状態確認に切り替える |

ここだけは変更内容に基づいて判断する。通常の修正は patch、利用者向けの CLI 機能追加は minor、互換性を壊す変更は major。`feat:` だけで判断せず、配布・インストール手順や開発補助の追加だけでは CLI の新機能と扱わない。ユーザー指定のバージョンを優先し、対象変更と採用理由を短く伝える。

## 2. バージョン更新とローカル検証

既に同じバージョンで準備中なら、そのブランチから再開する。対象の main に予定バージョンが反映済みなら手順4へ進む。

| コマンド・操作 | 成功条件 |
| --- | --- |
| `git switch -c BRANCH origin/main` | 作成成功。名前が既存の場合はそのブランチの差分を確認し、同じ準備だけなら `git switch BRANCH` で再利用 |
| `Cargo.toml` の `package.version` を `VERSION` に置換 | バージョン以外を変更しない |
| `cargo check` | 終了コード `0`。`Cargo.lock` を同期 |
| `make check` | 終了コード `0` |
| `make build` | 終了コード `0` |
| `target/release/giclean --version` | 出力が `giclean VERSION` |
| `git diff --check` | 終了コード `0` |
| `git diff --stat` | 変更が `Cargo.toml`・`Cargo.lock` のバージョン更新だけ |
| `git add Cargo.toml Cargo.lock` | 終了コード `0` |
| `git diff --cached --name-only` | 上記2ファイルだけ。既存の無関係なステージ済み変更があれば停止 |
| `git commit -m 'chore: vVERSION のリリース準備'` | 終了コード `0` |
| `git rev-parse HEAD` | `PREPARE_COMMIT` を記録 |

削除の動作確認は既存リポジトリでは実行しない。

## 3. PR による main への反映

| コマンド | 成功条件・分岐 |
| --- | --- |
| `git push -u origin BRANCH` | 終了コード `0`。force push は使わない |
| `gh pr list --repo kosnu/giclean --head BRANCH --base main --state all --json number,state,headRefOid,url` | 0件なら作成。1件なら同じ準備コミットか確認して再利用。マージ済みなら手順4へ。closed または複数件なら停止 |
| `gh pr create --repo kosnu/giclean --base main --head BRANCH --title 'chore: vVERSION のリリース準備' --body 'パッケージとロックファイルのバージョンを VERSION に揃える。検証: make check、make build、バージョン表示。'` | `PR_NUMBER` と URL を記録。本文の検証は実行済みのものだけ記載 |
| `gh pr checks PR_NUMBER --repo kosnu/giclean --watch --interval 10` | 終了コード `0` |
| `gh pr view PR_NUMBER --repo kosnu/giclean --json state,headRefOid,baseRefName,mergeStateStatus,reviewDecision,reviews` | open、head が `PREPARE_COMMIT`、base が main、マージ可能、変更要求なし |
| `gh api --paginate repos/kosnu/giclean/pulls/PR_NUMBER/comments` | 指摘が0件なら進む。存在する場合はスレッドの解決状態を調べ、未対応の指摘があれば停止。レビュー全体の状態だけで解決済みとみなさない |
| `gh pr merge PR_NUMBER --repo kosnu/giclean --squash --match-head-commit PREPARE_COMMIT` | 終了コード `0`。保護や必要レビューを迂回しない |

PR コメントではコミット ID をバッククォートで囲まない。

## 4. 公開するコミットとタグの確定

| コマンド | 成功条件・分岐 |
| --- | --- |
| `git fetch origin main --tags` | 終了コード `0` |
| `git rev-parse origin/main` | `RELEASE_COMMIT` を記録 |
| `git diff --exit-code PREPARE_COMMIT RELEASE_COMMIT` | 準備 PR を経由した場合だけ実行。終了コード `0`。別の変更が追加されていれば対象を再確認して検証をやり直す |
| `python3 .agents/skills/release/scripts/check_release.py source VERSION RELEASE_COMMIT` | 終了コード `0`。main の manifest と lock がともに `VERSION` |
| `gh run list --repo kosnu/giclean --workflow ci.yml --branch main --commit RELEASE_COMMIT --event push --json databaseId,headSha,status,conclusion` | 対象が1件なら `CI_RUN_ID` を記録 |
| `gh run watch CI_RUN_ID --repo kosnu/giclean --exit-status --interval 10` | 終了コード `0` |
| `git ls-remote --tags origin 'refs/tags/vVERSION*'` | タグがないなら新規作成。同名タグがあれば移動・再作成せず手順5で公開状態を検証 |
| `git tag -a vVERSION RELEASE_COMMIT -m 'giclean vVERSION'` | 新規の場合だけ実行。既存のローカルタグがあれば参照先と注釈付きであることを確認し、一致時だけ再利用 |
| `git push origin vVERSION` | 新規公開の場合だけ実行。そのタグだけを push |

## 5. Release 待機と公開確認

| コマンド | 成功条件・分岐 |
| --- | --- |
| `gh run list --repo kosnu/giclean --workflow release.yml --branch vVERSION --commit RELEASE_COMMIT --event push --json databaseId,headSha,status,conclusion` | 対象が1件なら `RUN_ID` を記録 |
| `gh run watch RUN_ID --repo kosnu/giclean --exit-status --interval 10` | 終了コード `0` |
| `python3 .agents/skills/release/scripts/check_release.py published VERSION RELEASE_COMMIT --run-id RUN_ID` | 終了コード `0` と公開 URL の出力で完了 |

実行 ID の検索が0件の場合は10秒間隔で最大6回まで再検索し、まだ0件なら停止。複数件なら自動で最新を選ばず停止する。

[確認スクリプト](scripts/check_release.py) は Python 3.11 以降の標準ライブラリと `git`・`gh` を使い、ソースのバージョン、注釈付きリモートタグの参照先、Release の実行 ID・SHA・タグ・成功状態、公開設定、リリースノート、4種類の配布ファイルと全チェックサムを検証する。ダウンロードは一時ディレクトリ内だけで行い、タグ・PR・リリースは変更しない。

失敗時は `gh run view RUN_ID --repo kosnu/giclean --log-failed` でログを取得して停止する。実行 ID 確定前や API・チェックサムの失敗では、その失敗を報告する。手元から `gh release create` で補完しない。公開済みタグの移動・再利用、リリースの削除・差し替えは行わず、修正は新しいバージョンで扱う。

最後にバージョン、公開 URL、主な変更、確認スクリプトの結果を報告する。未完了なら最後に成功した工程と失敗コマンドを示す。crates.io には公開しない。

スクリプト変更時の検証: `python3 -B -m unittest discover -s .agents/skills/release/scripts -p 'test_*.py' -v`。テストは通信や公開操作を行わない。
