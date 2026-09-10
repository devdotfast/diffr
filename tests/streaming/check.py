#!/usr/bin/env python3
"""Exercise the CLI stream against real Git repositories and partial staging."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "target/debug/diffr"
ENV = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1",
           GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.invalid",
           GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="test@example.invalid")

def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], env=ENV).decode().strip()

def commit(repo, message):
    git(repo, "add", ".")
    git(repo, "commit", "-qm", message)
    return git(repo, "rev-parse", "HEAD")

def cli(repo, *args):
    return subprocess.run([str(EXE), "--repo", str(repo), *args], capture_output=True, env=ENV)

def stream(repo, *args, code=0):
    with subprocess.Popen([str(EXE), "--repo", str(repo), "--format", "ndjson", *args],
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=ENV) as process:
        first = json.loads(process.stdout.readline())
        assert first["type"] == "start" and first["version"] == 1
        assert len(first["files"]) == first["total"]
        events = [first, *[json.loads(line) for line in process.stdout]]
        stderr = process.stderr.read()
        assert process.wait(timeout=30) == code, stderr
    assert events[-1]["type"] == "complete"
    succeeded = sum(e["type"] == "file" for e in events)
    failed = sum(e["type"] == "file_error" for e in events)
    assert events[-1] == dict(type="complete", succeeded=succeeded, failed=failed)
    assert succeeded + failed == first["total"]
    assert all("layout" not in e for e in events)
    return events

with tempfile.TemporaryDirectory(prefix="diffr-stream-") as temp:
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
    (repo / ".gitattributes").write_text("*.rs diffr-classify=source\n*.py diffr-classify=test\n*.bin diffr-classify=generated\n")
    (repo / "diffr.toml").write_text('[languages.rust]\nfolds = ""\n')
    events = stream(repo, base, head, "--order", "test,source,generated", code=2)
    assert events[0]["before"] == dict(kind="revision", ref=base)
    assert events[0]["after"] == dict(kind="revision", ref=head)
    assert [e["file"]["class"] for e in events[1:-1]] == ["test", "test", "test", "source", "generated"]
    renamed = next(e["file"] for e in events[1:-1] if e["file"]["status"] == "renamed")
    assert renamed["old_path"] == "rename.py" and renamed["new_path"] == "renamed.py"
    rust = next(e for e in events[1:-1] if e["file"]["new_path"] == "a.rs")
    assert rust["diff"]["rhs_folds"] == []
    # An early file failure must not prevent the later successes.
    events = stream(repo, base, head, "--order", "generated", code=2)
    assert events[1]["type"] == "file_error" and events[-1]["succeeded"] == 4
    events = stream(repo, base, head, "--", "a.rs", "z.py")
    assert [e["file"]["new_path"] for e in events[1:-1]] == ["a.rs", "z.py"]
    assert len(stream(repo, head, head)) == 2
    assert len(stream(repo, base, head, "--", "missing.rs")) == 2
    stream(repo, base, head, "--exit-code", "--", "a.rs", code=1)
    for args in (["bad-ref", head], [base, head, "--quiet"], ["--no-index", "a", "b"], [base, head, "--stat"]):
        result = cli(repo, "--format", "ndjson", *args)
        assert result.returncode == 2 and not result.stdout and result.stderr
    (repo / "diffr.toml").write_text("invalid toml")
    assert cli(repo, "--format", "ndjson", base, head).returncode == 2
    custom = repo / "custom.toml"
    custom.write_text("")
    events = stream(repo, base, head, "--config", str(custom), "--", "a.rs")
    assert events[1]["diff"]["rhs_folds"]

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
    for selection, left, right in (([], staged, working), (["--cached"], initial, staged), ([base], initial, working)):
        for reverse in (False, True):
            args = [*selection, *(["-R"] if reverse else [])]
            for output in ("--name-only", "--numstat"):
                actual = cli(repo, *args, output)
                expected = subprocess.check_output(["git", "-C", str(repo), "diff", *args, output], env=ENV)
                assert actual.returncode == 0 and actual.stdout == expected
            diff = stream(repo, *args)[1]["diff"]
            assert diff["lhs_src"]["Text"] == (right if reverse else left)
            assert diff["rhs_src"]["Text"] == (left if reverse else right)
    assert cli(repo, "--quiet").returncode == 1
    assert cli(repo, base, "HEAD", "--exit-code").returncode == 0
    git(repo, "rm", "-f", "a.rs")
    source.write_text(working)
    assert stream(repo, base)[1]["file"]["status"] == "deleted"
    assert cli(repo, base, "--name-status").stdout == b"D\ta.rs\n"

with tempfile.TemporaryDirectory(prefix="diffr-unborn-") as temp:
    repo = Path(temp)
    git(repo, "init", "-q")
    (repo / "new.rs").write_text("fn new() {}\n")
    git(repo, "add", ".")
    events = stream(repo, "--cached")
    assert events[0]["before"] == dict(kind="empty_tree")
    assert events[1]["file"]["status"] == "added"
# Closing the pipe while a multi-file producer is active must not leave it
# blocked forever on a full queue. Unix CLI output retains normal SIGPIPE behavior.
if os.name == "posix":
    with tempfile.TemporaryDirectory(prefix="diffr-cancel-") as temp:
        repo = Path(temp)
        git(repo, "init", "-q")
        git(repo, "commit", "--allow-empty", "-qm", "empty")
        base = git(repo, "rev-parse", "HEAD")
        for index in range(8):
            (repo / f"{index}.txt").write_text("some new text\n" * 4096)
        head = commit(repo, "large additions")
        with subprocess.Popen([str(EXE), "--repo", str(repo), base, head, "--format", "ndjson"],
                              stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=ENV) as process:
            assert json.loads(process.stdout.readline())["type"] == "start"
            process.stdout.close()
            assert process.wait(timeout=30) != 0
print("CLI streaming, cancellation and Git operand checks passed")
