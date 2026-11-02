# 設計

## Git コマンドを使う理由

Rust の標準ライブラリから Git コマンドを引数配列で起動します。シェルへのコマンド文字列の埋め込みは行いません。

Git の認証、credential helper、SSH 設定、worktree 保護、ブランチ設定の削除を既存の Git に任せるため、`git2` / `gix` は採用していません。Rust の外部クレートや GitHub CLI、GitHub API トークンは不要です。

## リモート照会と履歴判定

`br` は `git ls-remote --symref --exit-code origin HEAD` で既定ブランチ名と最新コミット ID をまとめて取得します。元の `gitcleanbr` の `gh repo view` と `git ls-remote` の二段階を、Git のリモート照会一回にしています。[Git の `--symref` 仕様](https://git-scm.com/docs/git-ls-remote#Documentation/git-ls-remote.txt---symref)に基づく構成です。

不足オブジェクトの自動取得を防ぐため、Git に `--no-lazy-fetch` を指定します。対応していない Git は処理開始時に失敗します。ローカルの replacement object でマージ判定が変わらないように、`GIT_NO_REPLACE_OBJECTS=1` も指定します。

Rust 化だけによる高速化や、実環境での速度向上は未計測です。問い合わせの集約と SSH 接続の再利用は別の改善として扱います。

## SSH 接続

通信は Git が選ぶ SSH 実装・コマンドに従います。OpenSSH を使用する場合、既存の `ControlMaster` / `ControlPersist` 設定を利用できます。例えば利用者が `~/.ssh/config` に設定する場合は以下の形です。

```sshconfig
Host github.com
    ControlMaster auto
    ControlPersist 60
    ControlPath ~/.ssh/cm-%C
```

ホスト名は実際の接続先に合わせてください。設定の意味は [OpenSSH 公式マニュアル](https://man.openbsd.org/ssh_config#ControlMaster)を参照してください。初回接続の待ち時間は残ります。

`giclean` 自身は接続を常駐管理せず、SSH 設定やシェル設定も書き換えません。

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
