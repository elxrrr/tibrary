#!/bin/sh
PROJECT_ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd -P)" || exit 1
exec "$PROJECT_ROOT/Start.command" --demo "$@"
