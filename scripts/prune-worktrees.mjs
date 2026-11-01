// Removes worktrees and local branches whose work is already on origin/main.
// A branch counts as landed when every commit on it has a twin on
// origin/main (git cherry), so squash merges and rewritten history count
// too. Worktrees with uncommitted changes, the main checkout and the
// current worktree are kept. Lists what would go unless --yes is passed
//
//   npm run prune             # dry run
//   npm run prune -- --yes    # remove them

import { execFileSync } from "node:child_process";
import { resolve } from "node:path";

const apply = process.argv.includes("--yes");
const git = (...args) => execFileSync("git", args, { encoding: "utf8" }).trim();
const tryGit = (...args) => {
  try {
    return git(...args);
  } catch {
    return null;
  }
};

const landed = (rev) =>
  tryGit("merge-base", "--is-ancestor", rev, "origin/main") !== null ||
  !git("cherry", "origin/main", rev).split("\n").some((line) => line.startsWith("+"));

git("fetch", "--prune", "--quiet", "origin");

const here = resolve(git("rev-parse", "--show-toplevel"));
const worktrees = git("worktree", "list", "--porcelain")
  .split("\n\n")
  .map((block) => ({
    path: block.match(/^worktree (.+)$/m)[1],
    branch: block.match(/^branch refs\/heads\/(.+)$/m)?.[1],
  }));
const [main, ...linked] = worktrees;

const removed = new Set();
for (const wt of linked) {
  if (resolve(wt.path) === here) continue;
  const status = tryGit("-C", wt.path, "status", "--porcelain");
  if (status === null) continue;
  if (status) {
    console.log(`keep   ${wt.path} (uncommitted changes)`);
    continue;
  }
  if (!landed(git("-C", wt.path, "rev-parse", "HEAD"))) {
    console.log(`keep   ${wt.path} (${wt.branch ?? "detached"} has unmerged commits)`);
    continue;
  }
  console.log(`remove ${wt.path}`);
  if (apply) git("worktree", "remove", wt.path);
  removed.add(wt.path);
}
if (apply) git("worktree", "prune");

const checkedOut = new Set(worktrees.filter((wt) => !removed.has(wt.path)).map((wt) => wt.branch));
for (const branch of git("for-each-ref", "--format=%(refname:short)", "refs/heads").split("\n")) {
  if (branch === "main" || checkedOut.has(branch) || !landed(branch)) continue;
  console.log(`delete ${branch}`);
  if (apply) git("branch", "-D", branch);
}

if (!apply) console.log("\nDry run. Pass --yes to remove these");
if (main.branch !== "main") console.log(`\nThe main checkout is on ${main.branch ?? "a detached HEAD"}, not main`);
