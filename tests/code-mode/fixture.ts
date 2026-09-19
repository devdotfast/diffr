import { execFile } from "node:child_process";
import { cp, mkdtemp, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import type { Scope, Hit } from "diffr/api";

const exec = promisify(execFile);
const fixtures = join(dirname(fileURLToPath(import.meta.url)), "fixtures");

export async function createFixture(): Promise<{ scope: Scope; cleanup(): Promise<void> }> {
  const temporary = await mkdtemp(join(tmpdir(), "diffr-code-mode-"));
  const repo = join(temporary, "repo");
  const git = async (...args: string[]) => (await exec("git", [
    "-c", "user.name=Code mode fixture",
    "-c", "user.email=fixture@example.invalid",
    "-c", "commit.gpgSign=false",
    "-c", "core.hooksPath=/dev/null",
    ...args,
  ], { cwd: repo })).stdout.trim();

  await cp(join(fixtures, "base"), repo, { recursive: true });
  await git("init", "--quiet");
  await git("add", ".");
  await git("commit", "--quiet", "-m", "Base fixture");
  const baseCommitId = await git("rev-parse", "HEAD");

  for (const name of await readdir(repo)) {
    if (name !== ".git") await rm(join(repo, name), { recursive: true });
  }
  await cp(join(fixtures, "head"), repo, { recursive: true });
  await git("add", "--all");
  await git("commit", "--quiet", "-m", "Head fixture");
  const headCommitId = await git("rev-parse", "HEAD");

  const scope = {
    repo,
    baseWorktree: { commitId: baseCommitId, path: join(temporary, "base") },
    headWorktree: { commitId: headCommitId, path: join(temporary, "head") },
  };
  await git("worktree", "add", "--quiet", "--detach", scope.baseWorktree.path, baseCommitId);
  await git("worktree", "add", "--quiet", "--detach", scope.headWorktree.path, headCommitId);

  return {
    scope,
    cleanup: () => rm(temporary, { recursive: true, force: true }),
  };
}

export async function grep(scope: Scope, token: string): Promise<Hit[]> {
  const { stdout } = await exec("rg", [
    "--no-config", "--json", "--fixed-strings", "--", token,
    scope.baseWorktree.path, scope.headWorktree.path,
  ]).catch(error => {
    if (error.code === 1) return { stdout: "" };
    throw error;
  });
  return stdout.split("\n").filter(Boolean)
    .map(line => JSON.parse(line))
    .filter(event => event.type === "match")
    .map(({ data }) => ({
      file: data.path.text,
      lines: [{ line: data.line_number, text: data.lines.text.replace(/\r?\n$/, "") }],
    }));
}
