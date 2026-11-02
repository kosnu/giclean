# 設計

## Git コマンドを使う理由

Rust の標準ライブラリから Git コマンドを引数配列で起動します。SSH 接続共有に使うコマンド文字列は、固定オプションと検証済みのユーザー ID・URL のハッシュ値で組み立て、リモート URL などの入力を直接埋め込みません。

Git の認証、credential helper、SSH 設定、worktree 保護、ブランチ設定の削除を既存の Git に任せるため、`git2` / `gix` は採用していません。Rust の外部クレートや GitHub CLI、GitHub API トークンは不要です。

## リモート照会と履歴判定

`br` は `git ls-remote --symref --exit-code origin HEAD` で既定ブランチ名と最新コミット ID をまとめて取得します。元の `gitcleanbr` の `gh repo view` と `git ls-remote` の二段階を、Git のリモート照会一回にしています。[Git の `--symref` 仕様](https://git-scm.com/docs/git-ls-remote#Documentation/git-ls-remote.txt---symref)に基づく構成です。

不足オブジェクトの自動取得を防ぐため、Git に `--no-lazy-fetch` を指定します。対応していない Git は処理開始時に失敗します。ローカルの replacement object でマージ判定が変わらないように、`GIT_NO_REPLACE_OBJECTS=1` も指定します。

問い合わせの集約と SSH 接続の再利用は別の改善として扱います。性能検証では、接続がない初回と接続を再利用できる実行を分け、同じリポジトリ・候補条件で比較します。Rust 化だけで通信待ちがなくなるとは扱いません。実測値と条件は [速度検証](performance.md)に記載しています。

## SSH 接続

macOS / Linux で通常の OpenSSH 6.7 以降を使う SSH リモートには、照会時に次のオプションを自動適用します。

- `ControlMaster=auto`：接続があれば再利用し、なければ接続を作ります。
- `ControlPersist=60`：最後の利用から60秒間、OpenSSH が接続を保持します。
- `ControlPath=/tmp/giclean-ssh-<uid>/cm-<url-hash>-%C`：利用者とリモート URL ごとに共有先を分けます。`%C` は接続先のホスト・ポート・ユーザーなどを区別します。

制御ソケットのディレクトリは所有者本人だけがアクセスできる `0700` で作成し、所有者・権限・シンボリックリンクでないことを確認します。接続共有は OpenSSH が管理し、Rust の常駐プロセスは起動しません。

`GIT_SSH_COMMAND` / `GIT_SSH` / `GIT_SSH_VARIANT`、Git の `core.sshCommand` / `ssh.variant` が指定済みの場合は、自動適用せず、その指定に従います。OpenSSH が対応していない場合や安全なソケットディレクトリを用意できない場合も、通常の Git 接続を使います。HTTPS やローカルリモートには適用しません。

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
