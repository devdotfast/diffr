#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# ///
"""Reference diffr fold hook: rewrite large novel folds as Python-style pseudocode.

diffr writes one NDJSON request per file on stdin and reads one reply per request
on stdout, matched by id. Requests are answered concurrently, so replies arrive
in whatever order the model finishes; diffr routes them by id.

Environment:
  OPENROUTER_API_KEY     required
  DIFFR_SUMMARY_MODEL    default google/gemini-3.1-flash-lite
  DIFFR_SUMMARY_WORKERS  concurrent requests, default 16
  DIFFR_SUMMARY_REASONING  "off" (default) disables model reasoning; "required" leaves
                           it on for models whose endpoint refuses to disable it
"""
import json
import os
import sys
import threading
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor

API_KEY = os.environ["OPENROUTER_API_KEY"]
MODEL = os.environ.get("DIFFR_SUMMARY_MODEL", "google/gemini-3.1-flash-lite")
WORKERS = int(os.environ.get("DIFFR_SUMMARY_WORKERS", "16"))
REASONING = os.environ.get("DIFFR_SUMMARY_REASONING", "off")
if REASONING not in ("off", "required"):
    raise SystemExit(f"DIFFR_SUMMARY_REASONING must be off or required, not {REASONING!r}")
URL = "https://openrouter.ai/api/v1/chat/completions"

SYSTEM = (
    "You rewrite regions of a source file as terse Python-style pseudocode for a diff "
    "viewer that shows the pseudocode in place of the collapsed region. The user supplies "
    "one numbered source file and a list of folds, each with an id and 1-based line range. "
    "For each fold, write pseudocode covering only that fold's lines: keep the control flow "
    "and the names that matter, drop types, error plumbing and boilerplate. Aim for about one "
    "pseudocode line per five source lines, between one and eight lines per fold. Reply with a "
    "JSON object mapping each fold id, as a string such as \"3\", to its pseudocode string."
)

write_lock = threading.Lock()


def prompt(request):
    numbered = "\n".join(
        f"{number:5d} | {line}" for number, line in enumerate(request["src"].splitlines(), 1)
    )
    folds = "\n".join(
        f"- fold {fold['id']}: lines {fold['range']['start']['line'] + 1}-"
        f"{fold['range']['end']['line'] + 1}"
        for fold in request["folds"]
    )
    language = request["language"] or "unknown language"
    return f"File {request['path']} ({language}):\n\n{numbered}\n\nFolds:\n{folds}"


def complete(request):
    body = {
        "model": MODEL,
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": prompt(request)},
        ],
        "temperature": 0,
        "max_tokens": 160 * len(request["folds"]) + 100,
        "response_format": {"type": "json_object"},
        "provider": {"sort": "latency"},
    }
    if REASONING == "off":
        body["reasoning"] = {"enabled": False}
    http = urllib.request.Request(
        URL,
        data=json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {API_KEY}", "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(http, timeout=60) as response:
        data = json.load(response)
    content = data["choices"][0]["message"]["content"]
    # Models vary between "3" and "fold 3" as keys; both identify fold 3.
    texts = {key.removeprefix("fold").strip(): text for key, text in json.loads(content).items()}
    expected = {str(fold["id"]) for fold in request["folds"]}
    unexpected = set(texts) - expected
    if unexpected or not all(isinstance(text, str) for text in texts.values()):
        raise ValueError(f"model returned malformed fold map: {content[:200]}")
    return {key: text.strip() for key, text in texts.items() if text.strip()}


def answer(line):
    request = json.loads(line)
    try:
        reply = {"id": request["id"], "texts": complete(request)}
    except urllib.error.HTTPError as error:
        reply = {"id": request["id"], "error": f"{MODEL}: HTTP {error.code} {error.read()[:200]!r}"}
    except (OSError, ValueError, KeyError) as error:
        reply = {"id": request["id"], "error": f"{MODEL}: {error}"}
    with write_lock:
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


def main():
    with ThreadPoolExecutor(WORKERS) as pool:
        for line in sys.stdin:
            if line.strip():
                pool.submit(answer, line)


if __name__ == "__main__":
    main()
