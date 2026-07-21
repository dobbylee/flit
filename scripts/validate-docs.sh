#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

node "$repo_dir/scripts/validate-docs.mjs"
git -C "$repo_dir" diff --check
