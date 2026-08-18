#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 1 || -z "$1" ]]; then
    echo "usage: $0 <destination-directory>" >&2
    exit 2
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST_DIR="$1"
THIRD_PARTY_DIR="$DEST_DIR/THIRD_PARTY_LICENSES"

mkdir -p "$THIRD_PARTY_DIR"
cp "$ROOT_DIR/LICENSE" "$DEST_DIR/LICENSE"
cp "$ROOT_DIR/assets/fonts/OFL-1.1.txt" "$THIRD_PARTY_DIR/OFL-1.1.txt"
cp "$ROOT_DIR/assets/fonts/SOURCE.zh-CN.md" \
    "$THIRD_PARTY_DIR/Noto-CJK-SOURCE.zh-CN.md"

test -s "$DEST_DIR/LICENSE"
test -s "$THIRD_PARTY_DIR/OFL-1.1.txt"
test -s "$THIRD_PARTY_DIR/Noto-CJK-SOURCE.zh-CN.md"
