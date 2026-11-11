#!/usr/bin/env bash
# Downloads logos-escrow's released guest binary to target/escrow, where
# logos-devnet.toml deploys it from, and checks it against its pinned sha256.
#
# usage: scripts/fetch_escrow.sh
set -euo pipefail
tag=v0.1.0
sha256=e39f35af4f8b93ee5e1ca0ecb6c4f524c43c4446a8a6ad56d8c7d3fde7c3deb5
dir="$(cd "$(dirname "$0")/.." && pwd)/target/escrow"
bin="$dir/logos_escrow.bin"

check() { [[ "$(shasum -a 256 "$1" | cut -d' ' -f1)" == "$sha256" ]]; }

if [[ -f "$bin" ]] && check "$bin"; then
  echo "$bin is logos-escrow $tag" >&2
  exit 0
fi
mkdir -p "$dir"
curl -fsSL -o "$bin.part" "https://github.com/drptbl/logos-escrow/releases/download/$tag/logos_escrow.bin"
if ! check "$bin.part"; then
  rm -f "$bin.part"
  echo "the downloaded logos_escrow.bin doesn't match logos-escrow $tag's sha256" >&2
  exit 1
fi
mv "$bin.part" "$bin"
echo "$bin is logos-escrow $tag" >&2
