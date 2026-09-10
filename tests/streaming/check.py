#!/usr/bin/env python3
"""Exercise the real server and client contract against temporary Git commits."""
import http.client
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
ENV = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1",
           GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.invalid",
           GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="test@example.invalid")

def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], env=ENV).decode().strip()

def commit(repo, message):
    git(repo, "add", ".")
    git(repo, "commit", "-qm", message)
    return git(repo, "rev-parse", "HEAD")

def revision(ref):
    return dict(kind="revision", ref=ref)

def comparison(base, head, **options):
    return dict(before=revision(base), after=revision(head), **options)

def request(port, body):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
    connection.request("POST", "/diff", json.dumps(body), {"Content-Type": "application/json"})
    response = connection.getresponse()
    if response.status != 200:
        data = json.loads(response.read())
        connection.close()
        return response.status, data
    assert response.getheader("Content-Type") == "application/x-ndjson"
    first = json.loads(response.readline())  # Parse before consuming the response.
    assert first["type"] == "start"
    events = [first]
    while line := response.readline():
        events.append(json.loads(line))
    connection.close()
    assert events[-1]["type"] == "complete"
    return 200, events

def serve(repo, config=None):
    args = [str(ROOT / "target/debug/difft"), "serve", "--repo", str(repo), "--listen", "127.0.0.1:0"]
    if config:
        args += ["--config", str(config)]
    process = subprocess.Popen(args, stderr=subprocess.PIPE, stdout=subprocess.DEVNULL, env=ENV, text=True)
    line = process.stderr.readline().strip()
    assert line.startswith("Listening on http://127.0.0.1:"), line
    return process, int(line.rsplit(":", 1)[1])

with tempfile.TemporaryDirectory(prefix="diffr-stream-test-") as temp:
    repo = Path(temp)
    git(repo, "init", "-q")
    (repo / "a.rs").write_text("fn run() { old(); }\n")
    (repo / "remove.py").write_text("print('remove')\n")
    (repo / "rename.py").write_text("print('same content')\n")
    base = commit(repo, "base")
    (repo / "a.rs").write_text("fn run() { old(); new(); }\n")
    (repo / "remove.py").unlink()
    (repo / "rename.py").rename(repo / "renamed.py")
    (repo / "binary.bin").write_bytes(b"a\0b")
    (repo / "z.py").write_text("print('new')\n")
    head = commit(repo, "head")
    # Uncommitted workspace attributes and config must apply to these pinned refs.
    (repo / ".gitattributes").write_text("*.rs diffr-classify=source\n*.py diffr-classify=test\n*.bin diffr-classify=generated\n")
    (repo / "diffr.toml").write_text('[languages.rust]\nfolds = ""\n')
    process, port = serve(repo)
    try:
        status, events = request(port, comparison(base=base, head=head, files=dict(order=["test", "source", "generated"])))
        assert status == 200
        assert events[0]["before"] == revision(base) and events[0]["after"] == revision(head)
        files = [e for e in events if e["type"] in ("file", "file_error")]
        assert len(files) == events[0]["total"] == 5
        assert [e["file"]["class"] for e in files] == ["test", "test", "test", "source", "generated"]
        assert events[-1] == dict(type="complete", succeeded=4, failed=1)
        assert files[-1]["type"] == "file_error"
        renamed = next(e["file"] for e in files if e["file"]["status"] == "renamed")
        assert renamed["old_path"] == "rename.py" and renamed["new_path"] == "renamed.py"
        rust = next(e for e in files if e["file"]["new_path"] == "a.rs")
        assert rust["diff"]["rhs_folds"] == [] and "layout" not in rust
        _, events = request(port, comparison(base=base, head=head, files=dict(order=["source"], paths=["a.rs", "z.py"])))
        assert events[1]["file"]["new_path"] == "a.rs"
        assert "layout" not in events[1]
        _, events = request(port, comparison(base=base, head=head, files=dict(order=["generated"])))
        assert events[1]["type"] == "file_error" and events[-1]["succeeded"] == 4
        _, events = request(port, comparison(base=base, head=head))
        _, empty = request(port, comparison(base=base, head=head, files=dict(order=[], paths=[])))
        assert empty == events, "empty file options select all changed files without class priority"
        names = [e["file"]["new_path"] or e["file"]["old_path"] for e in events[1:-1]]
        assert names == sorted(names), "omitted file order uses path order"
        _, events = request(port, comparison(base=head, head=head))
        assert [e["type"] for e in events] == ["start", "complete"]
        _, events = request(port, comparison(base=head, head=head, files=dict(paths=["a.rs"])))
        assert [e["type"] for e in events] == ["start", "complete"]
        assert request(port, comparison(base="missing-ref", head=head))[0] == 400
        assert request(port, comparison(base=base, head=head, files=dict(paths=["missing.rs"])))[0] == 200
        # The server keeps its compiled config even if the file changes.
        (repo / "diffr.toml").write_text("invalid toml")
        assert request(port, comparison(base=base, head=head, files=dict(paths=["a.rs"])))[0] == 200
    finally:
        process.terminate()
        process.wait(timeout=10)
    explicit = repo / "custom.toml"
    explicit.write_text('')
    process, port = serve(repo, explicit)
    try:
        _, events = request(port, comparison(base=base, head=head, files=dict(paths=["a.rs"])))
        assert events[1]["diff"]["rhs_folds"], "explicit config replaces repo config; default Rust folds survive"
    finally:
        process.terminate()
        process.wait(timeout=10)
print("Streaming integration checks passed")

# Compare real Git selections with the CLI, then verify the source content carried
# by the same comparisons over HTTP (particularly partial staging and reversal).
with tempfile.TemporaryDirectory(prefix="diffr-operands-") as temp:
    repo = Path(temp)
    git(repo, "init", "-q")
    source = repo / "a.rs"
    initial = "fn run() { initial(); }\n"
    staged = "fn run() { staged(); }\n"
    working = "fn run() { working(); }\n"
    source.write_text(initial)
    base = commit(repo, "initial")
    source.write_text(staged)
    git(repo, "add", "a.rs")
    source.write_text(working)

    def cli(*args):
        return subprocess.run([str(ROOT / "target/debug/difft"), "--repo", str(repo), *args],
                              capture_output=True, env=ENV)

    for selection in ([], ["--cached"], [base], [base, "HEAD"], ["-R"], ["--cached", "-R"], [base, "-R"]):
        for output in ("--name-only", "--numstat"):
            actual = cli(*selection, output)
            expected = subprocess.check_output(["git", "-C", str(repo), "diff", *selection, output], env=ENV)
            assert actual.returncode == 0, actual.stderr
            assert actual.stdout == expected, (selection, output, actual.stdout, expected)
    assert cli("--quiet").returncode == 1
    assert cli(base, "HEAD", "--exit-code").returncode == 0
    assert cli("--not-a-real-option").returncode == 2
    assert cli("--", "missing.rs").stdout == b""

    process, port = serve(repo)
    try:
        for before, after, lhs, rhs in (
            (dict(kind="index"), dict(kind="working_tree"), staged, working),
            (revision(base), dict(kind="index"), initial, staged),
            (revision(base), dict(kind="working_tree"), initial, working),
        ):
            for reverse in (False, True):
                a, b, left, right = (after, before, rhs, lhs) if reverse else (before, after, lhs, rhs)
                status, events = request(port, dict(before=a, after=b))
                assert status == 200, events
                diff = events[1]["diff"]
                assert diff["lhs_src"]["Text"] == left, diff["lhs_src"]
                assert diff["rhs_src"]["Text"] == right, diff["rhs_src"]
        # A staged deletion stays deleted in HEAD -> worktree even if an untracked
        # replacement exists at the same path.
        git(repo, "rm", "-f", "a.rs")
        source.write_text(working)
        _, events = request(port, dict(before=revision(base), after=dict(kind="working_tree")))
        assert events[1]["file"]["status"] == "deleted"
        assert cli(base, "--name-status").stdout == b"D\ta.rs\n"
    finally:
        process.terminate()
        process.wait(timeout=10)

with tempfile.TemporaryDirectory(prefix="diffr-unborn-") as temp:
    repo = Path(temp)
    git(repo, "init", "-q")
    (repo / "new.rs").write_text("fn new() {}\n")
    git(repo, "add", ".")
    result = subprocess.run([str(ROOT / "target/debug/difft"), "--repo", str(repo), "--cached", "--name-status"], capture_output=True, env=ENV)
    assert result.returncode == 0 and result.stdout == b"A\tnew.rs\n", result
print("Git operand checks passed")
