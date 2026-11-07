#!/bin/sh
# Fork patch (talhabw/hex): pull the latest upstream release, replay this
# fork's commits on top, build, and install over the running Hex.
#
#   scripts/fork-update.sh            # rebase on upstream/production (latest release)
#   scripts/fork-update.sh dev        # rebase on upstream/dev (bleeding edge)
#   scripts/fork-update.sh --no-rebase   # just rebuild and install this checkout
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
base=production
rebase=1
for arg in "$@"; do
  case "$arg" in
    --no-rebase) rebase= ;;
    *) base=$arg ;;
  esac
done

if [ -n "$(git status --porcelain)" ]; then
  echo "Commit or stash your changes first." >&2
  exit 1
fi

if [ -n "$rebase" ]; then
  git fetch upstream --tags
  echo "Rebasing $(git branch --show-current) onto upstream/$base..." >&2
  if ! git rebase "upstream/$base"; then
    echo "Rebase conflict. Fix it, 'git rebase --continue', then rerun with --no-rebase." >&2
    exit 1
  fi
  git push --force-with-lease origin HEAD
fi

# Keep the previous binary so a bad build can be rolled back by hand.
support=${HEX_APPLICATION_SUPPORT_DIR:-"$HOME/.local/share/hex"}
if [ -x "$support/current/hex" ]; then
  cp -f "$(readlink -f "$support/current/hex")" "$support/hex.previous"
fi

export CC=${CC:-clang} CXX=${CXX:-clang++}
"$root/scripts/install-linux.sh"
"$HOME/.local/bin/hex" restart
"$HOME/.local/bin/hex" --version
