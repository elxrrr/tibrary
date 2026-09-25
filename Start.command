#!/bin/sh
set -eu
PROJECT_ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)"
APP="$PROJECT_ROOT/desktop/src-tauri/target/release/bundle/macos/Tibrary.app"
if [ -d "$APP" ]; then
  exec /usr/bin/open "$APP"
fi
printf '%s\n' 'Build the Tauri application first:' '  npm --prefix desktop run tauri build' 'See desktop/docs/DOCUMENTATION.md for setup.'
exit 1
