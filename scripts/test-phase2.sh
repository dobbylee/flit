#!/bin/sh
set -eu

REPOSITORY_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$REPOSITORY_ROOT"

cargo test --locked --release -p flit-bridge --lib phase2_journey -- --nocapture
