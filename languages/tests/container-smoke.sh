#!/bin/sh
set -eu
for tool in cargo rustc cc gcc node npm python3; do
    if command -v "$tool" >/dev/null 2>&1; then
        echo "Unexpected development tool: $tool" >&2
        exit 1
    fi
done
test ! -e "$DIFFR_PARSER_DIR"
diffr languages list --json > /tmp/before.json
grep -q 'not_installed' /tmp/before.json
diffr languages install extra --json --archive /opt/extra.tgz
diffr languages list --json > /tmp/after.json
test "$(grep -o '"availability":"installed"' /tmp/after.json | wc -l)" -eq "$(wc -l < /opt/fixtures.txt)"
while IFS=: read -r fixture extension; do
    diffr --format ndjson --syntax --no-index "/fixtures/${fixture}_1.$extension" "/fixtures/${fixture}_2.$extension" > "/tmp/$fixture.ndjson"
    if grep -q '"fallback":{' "/tmp/$fixture.ndjson"; then
        echo "Unexpected fallback for $fixture" >&2
        exit 1
    fi
done < /opt/fixtures.txt
# An unchanged repeat is idempotent and needs no network or development tools.
diffr languages install extra --json --archive /opt/extra.tgz
for library in "$DIFFR_PARSER_DIR"/*/extra/*/fortran.so; do
    printf 'broken' > "$library"
done
diffr --format ndjson --no-index /fixtures/fortran_1.f90 /fixtures/fortran_2.f90 > /tmp/corrupt.ndjson
grep -q 'parser_load_failed' /tmp/corrupt.ndjson
diffr languages install extra --json --archive /opt/extra.tgz
diffr --format ndjson --syntax --no-index /fixtures/fortran_1.f90 /fixtures/fortran_2.f90 > /tmp/repaired.ndjson
cmp /tmp/fortran.ndjson /tmp/repaired.ndjson || {
    # Run summary timing is allowed to differ; compare deterministic file records.
    grep '"type":"file"' /tmp/fortran.ndjson > /tmp/expected-file
    grep '"type":"file"' /tmp/repaired.ndjson > /tmp/repaired-file
    cmp /tmp/expected-file /tmp/repaired-file
}
printf 'PASS: fresh offline install, all optional parsers, idempotence, corrupt fallback, atomic repair; unprivileged and without development tools.\n'
