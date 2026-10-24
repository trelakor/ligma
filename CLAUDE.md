# Ligma

## Branching

- Every new feature or fix goes on its own branch, never straight onto `main`.
- Before making the first edit, branch from an up-to-date `main`:
  `git switch main && git pull && git switch -c feature/<short-slug>` (use `fix/<short-slug>` for bug fixes).
- If you're already on a branch for that same work, keep using it. If you're on a branch for different work, ask before switching.
- Commit work to the branch and push it (`git push -u origin <branch>`). Open a PR against `main` only when asked. Don't merge it yourself.
- Release commits (`Release vX.Y.Z`) and tags are the one exception: they're made on `main` after the feature branch has been merged.
