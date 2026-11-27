#!/usr/bin/env bash
# Copy tessh's tree-sitter grammar from Tessera (its home) into vendor/.
set -euo pipefail
src="${1:-$HOME/src/tessera/tree-sitter-tessh}"
here="$(cd "$(dirname "$0")/.." && pwd)"
rsync -a --delete --exclude target --exclude node_modules "$src/" "$here/vendor/tree-sitter-tessh/"
echo "Synced $src -> vendor/tree-sitter-tessh"
