#!/usr/bin/env bash
# Checks emitted grammars against the real tree-sitter CLI.
#
# For every directory under the given root that holds a grammar.js and a
# grammar.json emitted by treesitter-lang:
#
#   1. `tree-sitter generate` must accept grammar.js;
#   2. the src/grammar.json it writes must equal our grammar.json byte for byte;
#   3. generating from our grammar.json must produce the same parser.c;
#   4. if the directory has a test/corpus, `tree-sitter test` must pass.
#
# Usage: dev/conformance.sh <root>    (needs `tree-sitter` on PATH, and a C
# compiler for step 4). The CI `tree-sitter` job runs it; see ci.yml.

set -euo pipefail

root="${1:?usage: dev/conformance.sh <root>}"
status=0

for dir in "$root"/*/; do
  name="$(basename "$dir")"
  echo "== $name"
  if ! (
    set -e
    cd "$dir"
    tree-sitter generate --js-runtime native
    cmp src/grammar.json grammar.json
    echo "   grammar.json matches tree-sitter's byte for byte"
    rm -rf from-json
    tree-sitter generate --js-runtime native -o from-json grammar.json
    cmp src/parser.c from-json/parser.c
    echo "   grammar.json and grammar.js generate the same parser"
    if [ -d test/corpus ]; then
      tree-sitter test
    fi
  ); then
    echo "   FAILED: $name"
    status=1
  fi
done

exit "$status"
