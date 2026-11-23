#!/usr/bin/env bash
# R1.1: the crate graph. Each crate may depend on the workspace crates listed here and no others. A new edge
# changes this file and docs/architecture.md together.
set -euo pipefail
cd "$(dirname "$0")/.."

declare -A ALLOWED=(
  [ai]=""
  [core]=""
  [attachments]="core"
  [plugins]=""
  [app-api]=""
  [tools]=""
  [telemetry]="ai"
  [providers]="core ai"
  [runtime]="core ai providers tools attachments"
  [memory]="ai core tools"
  [multiplayer]="ai core tools"
  [remote]="ai core providers runtime tools"
  [cli]="core providers runtime tools remote ai attachments"
  [shell]="memory multiplayer remote core ai telemetry providers runtime tools attachments"
)

fail=0
for dir in crates/*/; do
  name="$(basename "$dir")"
  manifest="$dir/Cargo.toml"
  [ -f "$manifest" ] || continue
  if [ -z "${ALLOWED[$name]+x}" ]; then
    echo "boundary: crate '$name' is not in scripts/check-boundaries.sh"
    fail=1
    continue
  fi
  # Workspace dependencies in [dependencies] only (not [dev-dependencies]).
  deps="$(awk '/^\[/{s=$0} s=="[dependencies]" && /^vivianne-/{sub(/^vivianne-/,""); split($0,a,/[ .=]/); print a[1]}' "$manifest")"
  for dep in $deps; do
    case " ${ALLOWED[$name]} " in
      *" $dep "*) ;;
      *) echo "boundary: crate '$name' depends on '$dep', which R1.1 does not allow"; fail=1 ;;
    esac
  done
done

# R1.7: every folder under crates/ and docs/ has a README.md.
while IFS= read -r d; do
  [ -f "$d/README.md" ] || { echo "readme: $d has no README.md"; fail=1; }
done < <(find crates docs -type d -not -path '*/target*' -not -path '*/node_modules*' -not -path '*/.git*')

exit "$fail"
