#!/bin/sh
PROJECT_ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)" || exit 1
cd "$PROJECT_ROOT" || exit 1
PYTHON_BIN="$(command -v python3.13)"
if [ -z "$PYTHON_BIN" ] && [ -x /opt/homebrew/bin/python3.13 ]; then
  PYTHON_BIN=/opt/homebrew/bin/python3.13
fi
if [ -z "$PYTHON_BIN" ]; then
  printf '%s\n' 'Python 3.13 is required for streaming downloads. Install it, then run this setup again.'
  if [ -t 0 ]; then
    printf '%s' 'Press Return to close.'
    read -r answer
  fi
  exit 1
fi
"$PYTHON_BIN" -m venv resources/tidaler/.venv || exit 1
resources/tidaler/.venv/bin/python -m pip install -e ./resources/tidaler || exit 1
if [ -t 0 ]; then
  printf '%s' 'Press Return to close.'
  read -r answer
fi
