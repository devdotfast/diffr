//! Revision-qualified `git grep -n` and `git grep -nz` records.
use super::{Hit, HitLine};
use anyhow::{ensure, Context};
use std::collections::BTreeMap;

pub(crate) fn parse(input: &[u8]) -> anyhow::Result<Vec<Hit>> {
    let input = std::str::from_utf8(input).context("Git-grep input must be UTF-8")?;
    let null = input.contains('\0');
    let mut remaining = input;
    let mut hits: BTreeMap<(String, String), BTreeMap<u32, String>> = BTreeMap::new();
    while !remaining.is_empty() {
        let (name, line, text, rest) = if null {
            let (name, rest) = remaining
                .split_once('\0')
                .context("missing Git-grep filename NUL")?;
            let (line, rest) = rest
                .split_once('\0')
                .context("missing Git-grep line-number NUL")?;
            let (text, rest) = record(rest);
            (name, line, text, rest)
        } else {
            let (row, rest) = record(remaining);
            // Filenames containing `:<digits>:` need `-z`.
            let (revision, tail) = row
                .split_once(':')
                .context("expected revision:path:line:text")?;
            let mut offset = None;
            for (i, _) in tail.match_indices(':') {
                if let Some((number, _)) = tail[i + 1..].split_once(':') {
                    if !number.is_empty() && number.bytes().all(|c| c.is_ascii_digit()) {
                        offset = Some((i, number.len()));
                        break;
                    }
                }
            }
            let (i, len) = offset.context("expected revision:path:line:text; use git grep -n")?;
            let name_len = revision.len() + 1 + i;
            (
                &row[..name_len],
                &tail[i + 1..i + 1 + len],
                &tail[i + len + 2..],
                rest,
            )
        };
        remaining = rest;
        let (revision, file) = name
            .split_once(':')
            .context("Git-grep hits must name a revision")?;
        ensure!(
            !revision.is_empty() && !file.is_empty(),
            "Git-grep revision and path must be nonempty"
        );
        ensure!(
            !line.is_empty() && line.bytes().all(|c| c.is_ascii_digit()),
            "invalid Git-grep line number"
        );
        let line: u32 = line.parse().context("Git-grep line number exceeds u32")?;
        ensure!(line > 0, "hit line numbers are 1-based");
        // Git emits the source CR before its own LF for CRLF files.
        let text = text.strip_suffix('\r').unwrap_or(text);
        let lines = hits
            .entry((revision.to_owned(), file.to_owned()))
            .or_default();
        if let Some(previous) = lines.insert(line, text.to_owned()) {
            ensure!(
                previous == text,
                "conflicting text for {revision}:{file}:{line}"
            );
        }
    }
    Ok(hits
        .into_iter()
        .map(|((revision, file), lines)| Hit {
            revision,
            file: file.into(),
            lines: lines
                .into_iter()
                .map(|(line, text)| HitLine { line, text })
                .collect(),
        })
        .collect())
}

fn record(input: &str) -> (&str, &str) {
    input.split_once('\n').unwrap_or((input, ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_text_and_deduplicates_hits() {
        let hits =
            parse(b"HEAD:src/a.js:9:  value:12: stays  \nHEAD:src/a.js:9:  value:12: stays  \n")
                .unwrap();
        assert_eq!(hits[0].lines.len(), 1);
        assert_eq!(hits[0].lines[0].text, "  value:12: stays  ");
        assert!(parse(b"HEAD:a:1:first\nHEAD:a:1:second\n").is_err());
    }

    #[test]
    fn rejects_unqualified_or_malformed_records() {
        for input in [
            "HEAD:a:4294967296:text",
            "\n",
            "HEAD:a\0x\0text\n",
            "HEAD:a\0",
            "HEAD:a:1:text\nHEAD:a\0",
        ] {
            assert!(parse(input.as_bytes()).is_err(), "{input:?}");
        }
    }
}
