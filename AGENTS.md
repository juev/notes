# Working in this repository

Rules for anyone changing this repository, human or agent. Build and test commands are in `README.md` and the `Makefile`; this file covers how the work is organised.

## One task, one worktree

Each task gets its own git worktree and its own branch. The main checkout stays on `main` and is not used for task work.

```sh
git fetch origin main
git worktree add -b <branch> ../notes-<branch> origin/main
```

- Name the branch after the change (`core-stage-1`, `fix-trash-expiry`), and put the worktree next to the repository as `../notes-<branch>`.
- Build outputs (`core/target`) are untracked and belong to the worktree.
- Do not switch branches inside a worktree to pick up another task, and do not edit files of one task from the worktree of another.
- Remove the worktree after its branch is merged: `git worktree remove ../notes-<branch>`.

## Specifications are updated with the code

The specifications in `docs/specs/` are the contract: `product.md`, `sync.md`, `editor.md` and `import.md`. A change in behaviour is not finished until the specification describes the new behaviour.

- Update the specification in the same branch as the code, and before the code when the change is a new requirement rather than a fix.
- A new requirement or scenario gets its own identifier in the existing numbering; tests refer to it by name (`s3_*`, `r28_*`).
- Comments in the code cite the requirements of Notes. A rule taken from Lists without a counterpart here is cited as "S<n> of Lists".
- When the change affects how the system is built, update `docs/architecture.md` and `README.md` as well.
- If the code and the specification disagree, find out which one is wrong before changing either. Do not edit the specification only to match what the code happens to do.
- The specifications and the rest of the documentation are written in English. Russian data in fixtures is quoted as it is.

## Changes come from investigation, not from guesses

Every change starts with finding out how things work now and ends with evidence that the change does what it claims.

1. Reproduce the problem, or for a new feature read the code and the specification it touches.
2. State the hypothesis: what is wrong or what is needed, and what observation would show it.
3. Test the hypothesis with something that can fail: a test, a log line, a query against the data folder.
4. Change the code only after the hypothesis is confirmed. When it is refuted, drop it and form the next one; do not stack speculative fixes.
5. Verify the result the same way, then run `make test`, `make lint` and, after a dependency change, `make deny`.

A conclusion put together from reading files is still a hypothesis until it has been traced end to end or reproduced. Report it as unverified, and say which checks were run and which were not.
