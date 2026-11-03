# 設計

## Git コマンドを使う理由

Rust の標準ライブラリから Git コマンドを引数配列で起動します。SSH 接続共有に使うコマンド文字列は、固定オプションと検証済みのユーザー ID・URL のハッシュ値で組み立て、リモート URL などの入力を直接埋め込みません。

Git の認証、credential helper、SSH 設定、worktree 保護、ブランチ設定の削除を既存の Git に任せるため、`git2` / `gix` は採用していません。Rust の外部クレートや GitHub CLI、GitHub API トークンは不要です。

macOS で PATH 上の Git が `/usr/bin/git` の場合だけ、`xcrun --find git` で同じ開発ツール設定が選ぶ Git 本体を実行中に一度解決します。ブランチごとの検査・削除で Apple の起動用プログラムを繰り返し通る時間を省きます。PATH 上で別の Git やラッパーが先に選ばれる場合はその指定を維持し、解決に失敗した場合も通常の `git` 起動に戻ります。Linux の起動方法は変更しません。

## リモート照会と履歴判定

`br` は `git ls-remote --symref --exit-code origin HEAD` で既定ブランチ名と最新コミット ID をまとめて取得します。元の `gitcleanbr` の `gh repo view` と `git ls-remote` の二段階を、Git のリモート照会一回にしています。[Git の `--symref` 仕様](https://git-scm.com/docs/git-ls-remote#Documentation/git-ls-remote.txt---symref)に基づく構成です。

不足オブジェクトの自動取得を防ぐため、Git に `--no-lazy-fetch` を指定します。対応していない Git は処理開始時に失敗します。ローカルの replacement object でマージ判定が変わらないように、`GIT_NO_REPLACE_OBJECTS=1` も指定します。

履歴上マージ済みでないブランチは、`git rev-parse --revs-only` に `先端OID...既定ブランチOID` をまとめて渡して共通祖先を確認します。[Git の三点記法](https://git-scm.com/docs/gitrevisions#_the_three_dot_symmetric_difference_notation)は `git merge-base --all` の全共通祖先を除外する展開で、共通祖先の計算自体は Git に任せます。先端 OID の重複を除き、128件ずつ処理して引数長を制限します。応答は要求した2つの OID と除外された共通祖先の組として検査します。

一意の共通祖先からブランチ先端への変更が空でなく、その全パスの変更が既定ブランチへの変更と一致する場合に候補へ加えます。コミットからツリー ID をまとめて取得し、`git diff-tree --stdin --raw -r -z --no-abbrev --no-renames` で複数のツリーペアを比較します。同じペアはバッチ内で一度だけ比較し、各応答のペア ID・NUL 区切りレコード・応答数を検査します。比較結果の再利用は実行中の固定 OID に限定し、128先端ごとに結果を破棄します。次回実行への参照情報のキャッシュはありません。

ツリー ID だけで判定できる場合は最新コミットとの差分を作りません。共通祖先と先端が同一ツリーなら正味の変更がないため保持し、共通祖先と既定ブランチが同一ツリーなら過去の一致判定へ進みます。どちらにも該当せず、先端と既定ブランチが同一ツリーなら変更全体が一致するため候補へ加えます。共通祖先の確認と操作直前の先端・worktree 使用状態の再確認は、この場合も行います。

最新コミットで一致しない先端は128先端のバッチ内で共通祖先ごとにまとめ、`git rev-list --format=%T --no-commit-header --first-parent --ancestry-path 共通祖先OID..既定ブランチOID` で過去のツリーを取得します。ツリー ID を検査し、共通祖先の子孫で、既定ブランチの第一親の履歴にある同一コミットで全変更パスが一致すれば候補へ加えます。マージ時に破棄された側の履歴だけの一致や、別々のコミットでの部分一致は対象にしません。最新コミットの判定で取得したブランチ側の差分を再利用し、128履歴ツリーずつ既存の raw 差分比較を使います。照会失敗・不正応答は削除前に停止します。過去に全変更が一致していれば、その後に更新・取り消しがあっても対象です。

履歴の異なるツリーが128個を超える場合は、範囲内で最も古い状態を先に完全一致で比較します。全先端が一致すれば残りの比較は不要です。一致しなかった先端に追加・変更されたファイルがあれば、必要時に一度だけ `git rev-list --objects --no-object-names 既定ブランチOID` で到達可能なオブジェクトを列挙します。これは対象履歴全体を含む集合なので、必要な blob が存在しなければ、その先端が一致する状態はありません。存在するだけでは取り込み済みとせず、従来の全パスの完全一致比較へ進めます。削除パスには blob を要求せず、列挙が追跡しない gitlink のコミット ID も要求しません。権限だけの変更で使う blob は共通祖先から到達可能なので、引き続き比較されます。履歴範囲や比較条件は省略・変更しません。

オブジェクト列挙の先頭が照会したコミット ID であることと、全 ID の形式を検査します。列挙失敗・不正応答は削除前に停止します。取得した ID は実行中だけ保持し、異なる共通祖先やバッチでも再利用します。次回実行へのキャッシュはありません。オブジェクト数が多いリポジトリでは、この列挙と集合の保持にも時間・メモリを使います。

完全なオブジェクト ID と権限を比較し、パッチの類似性は使いません。名前変更は削除と追加の両方が一致する必要があります。相対パス表示・外部 diff・textconv・submodule 差分の無視は明示的に無効にし、作業ツリーや index には触れません。[Git の raw 差分形式](https://git-scm.com/docs/git-diff-tree#_raw_output_format)に基づき、パスは NUL 区切りのバイト列として扱います。共通祖先やツリー差分の判定はすべて削除開始前に完了させ、各ブランチの先端と worktree 使用状態の再確認は操作直前に行います。

問い合わせの集約と SSH 接続の再利用は別の改善として扱います。性能検証では、接続がない初回と接続を再利用できる実行を分け、同じリポジトリ・候補条件で比較します。Rust 化だけで通信待ちがなくなるとは扱いません。実測値と条件は [速度検証](performance.md)に記載しています。

## 削除の一括処理

2〜128本、ブランチ名の引数が合計16KiB以内の候補は、`git branch -D` の一回の起動で削除します。Git の `reference-transaction` フックの `prepared` 段階で、ロック取得後の全候補の OID・HEAD・worktree 使用状態・symbolic 状態を別の Git プロセスで読み直します。初期 OID と一致し、保護対象に変わっていない場合だけ削除の確定を許可します。

フックには現在の実行ファイルへのシンボリックリンクを使用します。一時ディレクトリは PID・時刻・連番を含む名前で排他的に `0700` で作成し、期待する参照情報と検査結果を置きます。終了経路では一時領域を片付けます。`core.hooksPath` は削除コマンドに限定して指定します。既存の実行可能なフックがある場合は元の個別処理を使います。Git がフック実行前に変更する PATH に影響されないよう、検査に使う Git 実行先は親プロセスで絶対パスに解決します。

状態変化による拒否や参照ロック競合では元の個別処理に戻り、各枝を再確認します。検査の実行失敗・応答不正は削除中止として扱います。参照削除後のブランチ設定処理で失敗した場合も、削除済みの枝を結果から落とさず失敗として報告します。参照ロックと他プロセスの worktree 作成を共通のロックで管理する構成ではないため、最終確認後の同時操作を完全に防ぐ保証はありません。

## SSH 接続

macOS / Linux で通常の OpenSSH 6.7 以降を使う SSH リモートには、照会時に次のオプションを自動適用します。

- `ControlMaster=auto`：接続があれば再利用し、なければ接続を作ります。
- `ControlPersist=60`：最後の利用から60秒間、OpenSSH が接続を保持します。
- `SendEnv=GIT_PROTOCOL`：接続を再利用した場合も、その照会が選んだ Git プロトコルをサーバーへ渡します。
- `ControlPath=/tmp/giclean-ssh-<uid>/cm-<url-hash>-%C`：利用者とリモート URL ごとに共有先を分けます。`%C` は接続先のホスト・ポート・ユーザーなどを区別します。

制御ソケットのディレクトリは所有者本人だけがアクセスできる `0700` で作成し、所有者・権限・シンボリックリンクでないことを確認します。接続共有は OpenSSH が管理し、Rust の常駐プロセスは起動しません。

`GIT_SSH_COMMAND` / `GIT_SSH` / `GIT_SSH_VARIANT`、Git の `core.sshCommand` / `ssh.variant` が指定済みの場合は、自動適用せず、その指定に従います。OpenSSH が対応していない場合や安全なソケットディレクトリを用意できない場合も、通常の Git 接続を使います。HTTPS やローカルリモートには適用しません。

この自動適用時に `protocol.version` が未指定なら、リモート照会に限って Git のプロトコル v0 を指定します。v2 の能力交渉後の `ls-refs` 要求を省き、通信往復を一回減らします。v0 は全参照の広告を受信するため、参照が非常に多い環境では v2 の方が適する可能性があります。リポジトリ・グローバル設定・コマンドスコープの環境変数で `protocol.version` が明示されている場合は維持します。Git の設定ファイルやリモート設定は書き換えません。[プロトコルの選択](https://git-scm.com/docs/git-config#Documentation/git-config.txt-protocolversion)と [v2 の参照照会](https://git-scm.com/docs/protocol-v2#_ls_refs)は Git の公式仕様に従います。

鍵・ホスト鍵検証・ProxyJump など、通常の OpenSSH 設定は引き継ぎます。SSH 設定ファイルやシェル設定は変更しません。共有するのは接続だけで、既定ブランチ名・コミット ID は毎回リモートへ問い合わせます。初回接続の待ち時間は残ります。

### 既存の接続共有設定を使う場合

ツールによるオプションの自動適用を避けるには、SSH コマンドを明示します。

```sh
GIT_SSH_COMMAND=ssh giclean br --dry-run
```

この場合は通常の Git と同じ設定で接続します。例えば、ほかの Git 操作とも共有する設定を利用者の `~/.ssh/config` に置く場合は以下の形です。

```sshconfig
Host github.com
    ControlMaster auto
    ControlPersist 60
    ControlPath ~/.ssh/cm-%C
```

ホスト名は実際の接続先に合わせてください。設定の意味は [OpenSSH 公式マニュアル](https://man.openbsd.org/ssh_config#ControlMaster)、明示的な SSH コマンドの扱いは [Git の環境変数](https://git-scm.com/docs/git#Documentation/git.txt-codeGITSSHCOMMANDcode)を参照してください。

## gitcleanbr からの移行

既存の `gitcleanbr` 関数は自動置換しません。動作確認後に利用者の dotfiles 側で変更する場合は、次のように `br` を明示します。

```sh
gitcleanbr() { giclean br "$@"; }
```

## 背景資料

- [Git お掃除コマンドの Rust 化 引き継ぎ資料](https://chatgpt.com/space/page_9253f07aeb948191a99222be36b4ffd6)
- [元の gitcleanbr 実装](https://github.com/kosnu/dotfiles/blob/1fd56078ea2032d5e769613c0632f90623101785/.zshrc#L64)
- [Git: for-each-ref](https://git-scm.com/docs/git-for-each-ref)
- [Git: branch](https://git-scm.com/docs/git-branch)

引き継ぎ資料のライブラリ・接続共有案は検討事項として扱い、現行ソースと Git の仕様を確認して上記の構成にしています。コマンドの動作と制約は [br の仕様](commands/br.md)を参照してください。
