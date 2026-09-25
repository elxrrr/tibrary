#!/bin/sh
set -eu
PROJECT_ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd -P)"
APP="$PROJECT_ROOT/desktop/src-tauri/target/release/bundle/macos/Tibrary.app/Contents/MacOS/tibrary"
if [ ! -x "$APP" ]; then
  printf '%s\n' 'Build Tibrary first. See desktop/docs/DOCUMENTATION.md.'
  exit 1
fi
export TIBRARY_DEMO=1
exec "$APP"
