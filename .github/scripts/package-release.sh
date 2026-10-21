#!/usr/bin/env bash
# Package only the executable and its public user-facing files.
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "Usage: $0 <target> <binary> <output-directory>" >&2
  exit 1
fi

release_target=$1
release_binary=$2
release_output=$3
case "$release_target" in
  aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-musl) ;;
  *) echo "Unsupported release target: $release_target" >&2; exit 1 ;;
esac
if [[ ! -f "$release_binary" || ! -x "$release_binary" ]]; then
  echo "Release binary is missing or not executable: $release_binary" >&2
  exit 1
fi

script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
project_directory=$(cd -- "$script_directory/../.." && pwd)
mkdir -p "$release_output"
release_output=$(cd -- "$release_output" && pwd)
staging_directory=$(mktemp -d "${TMPDIR:-/tmp}/seqviz-release.XXXXXX")
trap 'rm -rf "$staging_directory"' EXIT

install -m 755 "$release_binary" "$staging_directory/sq"
cp "$project_directory/README.md" "$project_directory/README.zh-CN.md" "$project_directory/LICENSE" "$staging_directory/"
cp -R "$project_directory/config" "$project_directory/contrib" "$staging_directory/"
mkdir -p "$staging_directory/docs"
cp -R "$project_directory/docs/assets" "$staging_directory/docs/assets"

# Explicit members keep source, private design documents and local videos out.
archive="$release_output/seqviz-rs-$release_target.tar.gz"
COPYFILE_DISABLE=1 tar -czf "$archive" -C "$staging_directory" \
  sq README.md README.zh-CN.md LICENSE config contrib docs/assets
echo "$archive"
