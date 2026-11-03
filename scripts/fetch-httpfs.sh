#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Downloads DuckDB's signed httpfs extension, which database encryption needs,
# for the DuckDB version in Cargo.lock. Release builds ship it with the app;
# DuckDB checks its signature when it loads it.
#
#   scripts/fetch-httpfs.sh <rust-target> <output-file>
#   scripts/fetch-httpfs.sh x86_64-pc-windows-msvc apps/desktop/src-tauri/resources/duckdb/httpfs.duckdb_extension
set -euo pipefail

target=${1:?rust target, e.g. x86_64-unknown-linux-gnu}
out=${2:?output file}

case "$target" in
  aarch64-apple-darwin) platform=osx_arm64 ;;
  x86_64-apple-darwin) platform=osx_amd64 ;;
  x86_64-unknown-linux-gnu) platform=linux_amd64 ;;
  aarch64-unknown-linux-gnu) platform=linux_arm64 ;;
  x86_64-pc-windows-msvc) platform=windows_amd64 ;;
  *) echo "no DuckDB extension platform for $target" >&2; exit 1 ;;
esac

# libduckdb-sys 1.MMmmpp.x bundles DuckDB vMM.mm.pp (1.10506.0 → v1.5.6).
root=$(cd "$(dirname "$0")/.." && pwd)
crate=$(awk '/^name = "libduckdb-sys"$/ { getline; gsub(/version = |"/, ""); print; exit }' "$root/Cargo.lock")
n=$(echo "$crate" | cut -d. -f2)
[[ "$n" =~ ^[0-9]{5,}$ ]] || { echo "unexpected libduckdb-sys version: $crate" >&2; exit 1; }
version="v$((10#$n / 10000)).$(((10#$n / 100) % 100)).$((10#$n % 100))"

url="https://extensions.duckdb.org/$version/$platform/httpfs.duckdb_extension.gz"
echo "httpfs $version for $platform: $url"
mkdir -p "$(dirname "$out")"
curl -fsSL --retry 3 "$url" | gunzip > "$out.tmp"
mv "$out.tmp" "$out"
ls -l "$out"
