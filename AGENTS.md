Respond to the user in Japanese.

## Scope

- `giclean` is a CLI for Git cleanup. The `br` subcommand cleans local branches merged into the latest default branch of `origin`.
- Keep the runtime and tests dependency-free unless a requested feature requires a dependency.
- Use the installed Git executable so authentication, credential helpers, and SSH settings follow the user's Git configuration.
- Preserve the no-fetch behavior, dry-run output, protected branches, and worktree checks. Never infer squash merges from PRs or patch similarity.
- Run destructive verification only in temporary test repositories. Do not run branch deletion in this checkout or another existing repository as a smoke test.

## Verification

- Run `make check` and `make build` after behavior changes.
- Add integration coverage with real temporary repositories for changes to deletion eligibility or failure handling.
- CI runs the same checks on Linux and macOS. Do not claim remote CI success without checking the remote result.
- Do not modify shell configuration, SSH configuration, remotes, or publish releases unless requested.
