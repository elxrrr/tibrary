#!/bin/sh
PROJECT_ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)" || exit 1
cd "$PROJECT_ROOT/app" || exit 1
if [ ! -x "$PROJECT_ROOT/.venv/bin/python" ]; then
  printf '%s\n' 'Install dependencies using app/docs/reference/DEVELOPMENT.md first.'
  if [ -t 0 ]; then
    printf '%s' 'Press Return to close.'
    read -r answer
  fi
  exit 1
fi
exec "$PROJECT_ROOT/.venv/bin/python" -m library_manager "$@"
