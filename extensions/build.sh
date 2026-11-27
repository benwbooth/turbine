#!/usr/bin/env bash
# Build Turbine extensions (extensions/<name>) as WebAssembly components and
# install them where Turbine looks: $XDG_DATA_HOME/turbine/extensions/<name>.
#   extensions/build.sh [name...]      (all of them when none is given)
#   DEST=dir extensions/build.sh ...    (somewhere else)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
dest=${DEST:-${XDG_DATA_HOME:-$HOME/.local/share}/turbine/extensions}
names=("$@")
[ ${#names[@]} -gt 0 ] || names=($(cd "$here" && ls -d */ | tr -d /))
for n in "${names[@]}"; do
  echo "building $n"
  cargo build --quiet --manifest-path "$here/$n/Cargo.toml" --target wasm32-unknown-unknown --release
  core=$(ls "$here/$n/target/wasm32-unknown-unknown/release/"*.wasm | head -1)
  mkdir -p "$dest/$n"
  turbine pack-extension "$core" "$dest/$n/extension.wasm"
  cp "$here/$n/README.md" "$dest/$n/" 2>/dev/null || true
  echo "installed $dest/$n"
done
