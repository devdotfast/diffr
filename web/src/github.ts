/**
 * Read a pull request or a comparison straight from GitHub. Requests go only to api.github.com and
 * raw.githubusercontent.com, both of which allow cross-origin reads; the token, when there is one,
 * stays in this browser's localStorage and goes only to api.github.com.
 */

export type Target =
  | { kind: "pull"; owner: string; repo: string; number: number }
  | { kind: "compare"; owner: string; repo: string; base: string; head: string };

export type Status = "added" | "deleted" | "modified" | "renamed" | "copied";

export interface ChangedFile {
  path: string;
  /** The path at the base, for a rename or copy. */
  previousPath?: string;
  status: Status;
}

export interface Change {
  target: Target;
  title: string;
  url: string;
  /** The merge base, as GitHub's own PR diff uses: what the head is compared with. */
  base: string;
  head: string;
  files: ChangedFile[];
}

const TOKEN_KEY = "diffr.githubToken";

export function token(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export function setToken(value: string | null) {
  try {
    if (value) localStorage.setItem(TOKEN_KEY, value);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    // Storage is off: the token lasts until the page closes, which is never, since nothing keeps it.
  }
}

/** `owner/repo/pull/N` or `owner/repo/compare/base...head`, with or without github.com in front. */
export function parseTarget(input: string): Target | null {
  const path = input.trim().replace(/^https?:\/\/(www\.)?github\.com\//, "").replace(/^\/+/, "").replace(/[?#].*$/, "");
  const pull = path.match(/^([\w.-]+)\/([\w.-]+)\/pulls?\/(\d+)/);
  if (pull) return { kind: "pull", owner: pull[1]!, repo: pull[2]!, number: Number(pull[3]) };
  const compare = path.match(/^([\w.-]+)\/([\w.-]+)\/compare\/(.+?)\.\.\.?(.+)$/);
  if (compare)
    return { kind: "compare", owner: compare[1]!, repo: compare[2]!, base: decodeURIComponent(compare[3]!), head: decodeURIComponent(compare[4]!) };
  return null;
}

export function targetPath(target: Target): string {
  const repo = `/${target.owner}/${target.repo}`;
  return target.kind === "pull"
    ? `${repo}/pull/${target.number}`
    : `${repo}/compare/${encodeURIComponent(target.base)}...${encodeURIComponent(target.head)}`;
}

export class GitHubError extends Error {
  constructor(message: string, readonly status: number) {
    super(message);
  }
}

async function api<T>(path: string): Promise<T> {
  const auth = token();
  const response = await fetch(`https://api.github.com${path}`, {
    headers: { Accept: "application/vnd.github+json", ...(auth ? { Authorization: `Bearer ${auth}` } : {}) },
  });
  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as { message?: string };
    const limited = response.headers.get("x-ratelimit-remaining") === "0";
    const message = limited
      ? `GitHub's rate limit is spent${auth ? "" : " (60 requests an hour without a token)"}; add a token or try later`
      : response.status === 404 && !auth
        ? "Not found. A private repository needs a token"
        : body.message ?? response.statusText;
    throw new GitHubError(`GitHub ${response.status}: ${message}`, response.status);
  }
  return response.json() as Promise<T>;
}

interface ApiFile {
  filename: string;
  previous_filename?: string;
  status: "added" | "removed" | "modified" | "renamed" | "copied" | "changed" | "unchanged";
}

const statuses: Record<ApiFile["status"], Status> = {
  added: "added", removed: "deleted", modified: "modified", renamed: "renamed",
  copied: "copied", changed: "modified", unchanged: "modified",
};

const changedFile = (file: ApiFile): ChangedFile => ({
  path: file.filename,
  previousPath: file.previous_filename,
  status: statuses[file.status],
});

/** Every page of a listing GitHub caps at 100 a page; it stops listing a PR's files at 3000. */
async function pages<T>(path: (page: number) => string, items: (response: unknown) => T[]): Promise<T[]> {
  const all: T[] = [];
  for (let page = 1; page <= 30; page++) {
    const batch = items(await api(path(page)));
    all.push(...batch);
    if (batch.length < 100) break;
  }
  return all;
}

export async function loadChange(target: Target): Promise<Change> {
  const repo = `/repos/${target.owner}/${target.repo}`;
  if (target.kind === "pull") {
    const pr = await api<{ title: string; html_url: string; base: { sha: string }; head: { sha: string } }>(
      `${repo}/pulls/${target.number}`);
    const [compare, files] = await Promise.all([
      api<{ merge_base_commit: { sha: string } }>(`${repo}/compare/${pr.base.sha}...${pr.head.sha}?per_page=1`),
      pages((page) => `${repo}/pulls/${target.number}/files?per_page=100&page=${page}`, (r) => r as ApiFile[]),
    ]);
    return { target, title: pr.title, url: pr.html_url, base: compare.merge_base_commit.sha, head: pr.head.sha,
      files: files.map(changedFile) };
  }
  const range = `${encodeURIComponent(target.base)}...${encodeURIComponent(target.head)}`;
  type Compare = { merge_base_commit: { sha: string }; html_url: string; files?: ApiFile[] };
  let compare: Compare | undefined;
  const [head, files] = await Promise.all([
    api<{ sha: string }>(`${repo}/commits/${encodeURIComponent(target.head)}`),
    pages((page) => `${repo}/compare/${range}?per_page=100&page=${page}`, (r) => {
      compare ??= r as Compare;
      return (r as Compare).files ?? [];
    }),
  ]);
  return { target, title: `${target.base}...${target.head}`, url: compare!.html_url, base: compare!.merge_base_commit.sha,
    head: head.sha, files: files.map(changedFile) };
}

const encodePath = (path: string) => path.split("/").map(encodeURIComponent).join("/");

/** A file's text at a commit. Without a token, raw.githubusercontent.com serves public files outside the API's rate limit. */
export async function fileText(target: Target, sha: string, path: string): Promise<string> {
  const auth = token();
  const response = auth
    ? await fetch(`https://api.github.com/repos/${target.owner}/${target.repo}/contents/${encodePath(path)}?ref=${sha}`, {
        headers: { Accept: "application/vnd.github.raw+json", Authorization: `Bearer ${auth}` },
      })
    : await fetch(`https://raw.githubusercontent.com/${target.owner}/${target.repo}/${sha}/${encodePath(path)}`);
  if (!response.ok) throw new GitHubError(`GitHub ${response.status} fetching ${path}`, response.status);
  // diffr decides what is binary from the bytes, so keep NULs; invalid UTF-8 becomes U+FFFD.
  return new TextDecoder().decode(await response.arrayBuffer());
}
