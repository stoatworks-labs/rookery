#!/usr/bin/env bash
# release-local.sh — cut a full rookery release from this Mac.
#
# rookery is a headless server with an embedded web UI, so there is no Tauri
# tray launcher to build (RR_LAUNCHER stays empty) and no Windows VM step. CI
# builds the same artefacts from the same harness, so a local build and a
# tagged CI build are identical.
#
#   scripts/release-local.sh                  build into dist-release/
#   scripts/release-local.sh --upload         tag and publish the GitHub release
set -euo pipefail

RR_NAME="rookery"
RR_SLUG="rookery"
RR_IDENT="com.stoatworks.rookery"
RR_EXTRA_FILES=("README.md" "LICENSE")
RR_EXTRA_DIRS=("config" "docs")

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release-rust.sh"
