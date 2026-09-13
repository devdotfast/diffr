#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["aiohttp>=3.9", "httpx>=0.27", "jsonrpcserver>=5"]
# ///
"""Reference diffr fold hook: rewrite large novel folds as Python-style pseudocode.

diffr starts this server once per invocation with the port in DIFFR_HOOK_PORT and
calls the JSON-RPC 2.0 method `summarize` once per file, concurrently. Each call
awaits one Gemini request on a shared httpx client. Log to stderr; stdout is
discarded by diffr.

Environment:
  DIFFR_HOOK_PORT        set by diffr
  GOOGLE_API_KEY         required
  DIFFR_SUMMARY_MODEL    default gemini-3.8-flash
  DIFFR_SUMMARY_WORKERS  concurrent model requests, default 16
"""

import asyncio
import json
import os

import httpx
from aiohttp import web
from jsonrpcserver import Error, Result, Success, async_dispatch, method

API_KEY = os.environ["GOOGLE_API_KEY"]
PORT = int(os.environ["DIFFR_HOOK_PORT"])
MODEL = os.environ.get("DIFFR_SUMMARY_MODEL", "gemini-3.8-flash")
WORKERS = int(os.environ.get("DIFFR_SUMMARY_WORKERS", "16"))
URL = f"https://generativelanguage.googleapis.com/v1beta/models/{MODEL}:generateContent"

SYSTEM = (
    "You rewrite regions of a source file as terse Python-style pseudocode for a diff "
    "viewer that shows the pseudocode in place of the collapsed region. The user supplies "
    "one numbered source file and a list of folds, each with an id (the region's alignment_id) and 1-based line range. "
    "For each fold, write pseudocode covering only that fold's lines: keep the control flow "
    "and the names that matter, drop types, error plumbing and boilerplate. Aim for about one "
    "pseudocode line per five source lines, between one and eight lines per fold. Reply with "
    "one {id, pseudocode} object per fold."
)
SCHEMA = {
    "type": "ARRAY",
    "items": {
        "type": "OBJECT",
        "properties": {"id": {"type": "INTEGER"}, "pseudocode": {"type": "STRING"}},
        "required": ["id", "pseudocode"],
    },
}

client = httpx.AsyncClient(headers={"x-goog-api-key": API_KEY}, timeout=60)
limit = asyncio.Semaphore(WORKERS)


def prompt(path, language, src, folds):
    numbered = "\n".join(
        f"{n:5d} | {line}" for n, line in enumerate(src.splitlines(), 1)
    )
    ranges = "\n".join(
        f"- fold {fold['id']}: lines {fold['range']['start']['line'] + 1}-"
        f"{fold['range']['end']['line'] + 1}"
        for fold in folds
    )
    return f"File {path} ({language or 'unknown language'}):\n\n{numbered}\n\nFolds:\n{ranges}"


async def complete(path, language, src, folds):
    body = {
        "systemInstruction": {"parts": [{"text": SYSTEM}]},
        "contents": [
            {"role": "user", "parts": [{"text": prompt(path, language, src, folds)}]}
        ],
        "generationConfig": {
            "temperature": 0,
            "maxOutputTokens": 160 * len(folds) + 100,
            "thinkingConfig": {"thinkingBudget": 0},
            "responseMimeType": "application/json",
            "responseSchema": SCHEMA,
        },
    }
    async with limit:
        response = await client.post(URL, json=body)
    response.raise_for_status()
    content = response.json()["candidates"][0]["content"]["parts"][-1]["text"]
    expected = {fold["id"] for fold in folds}
    texts = {}
    for item in json.loads(content):
        if item["id"] not in expected:
            raise ValueError(
                f"model answered for unknown fold {item['id']}: {content[:200]}"
            )
        if item["pseudocode"].strip():
            texts[str(item["id"])] = item["pseudocode"].strip()
    return texts


@method
async def summarize(path, language, src, folds) -> Result:
    try:
        return Success(await complete(path, language, src, folds))
    except httpx.HTTPStatusError as error:
        return Error(
            -32000,
            f"{MODEL}: HTTP {error.response.status_code} {error.response.text[:200]}",
        )
    except (httpx.HTTPError, ValueError, KeyError) as error:
        return Error(-32000, f"{MODEL}: {error}")


async def handle(request: web.Request) -> web.Response:
    return web.Response(
        text=await async_dispatch(await request.text()), content_type="application/json"
    )


app = web.Application()
app.router.add_post("/", handle)

if __name__ == "__main__":
    web.run_app(app, host="127.0.0.1", port=PORT, print=None, access_log=None)
