#!/usr/bin/env bash
# Deterministic release-tag checks. Publication must not begin unless the tag,
# workspace version, changelog, and main ancestry all describe the same release.
set -euo pipefail

usage() {
  echo "usage: $0 --tag vX.Y.Z [--commit SHA] [--main-ref REF]" >&2
  exit 2
}

TAG=""
COMMIT="HEAD"
MAIN_REF="origin/main"
while [ $# -gt 0 ]; do
  case "$1" in
    --tag) TAG="${2:?--tag needs a value}"; shift 2 ;;
    --commit) COMMIT="${2:?--commit needs a value}"; shift 2 ;;
    --main-ref) MAIN_REF="${2:?--main-ref needs a value}"; shift 2 ;;
    *) usage ;;
  esac
done

[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]] || {
  echo "release tag must be a v-prefixed semantic version, got: $TAG" >&2
  exit 1
}
TAG_VERSION="${TAG#v}"

WORKSPACE_VERSION="$({
  in_workspace_package=0
  while IFS= read -r line; do
    if [ "$line" = "[workspace.package]" ]; then
      in_workspace_package=1
      continue
    fi
    if [[ "$line" == \[* ]]; then
      in_workspace_package=0
    fi
    if [ "$in_workspace_package" -eq 1 ] && [[ "$line" =~ ^version[[:space:]]*=[[:space:]]*\"([^\"]+)\" ]]; then
      printf '%s\n' "${BASH_REMATCH[1]}"
      break
    fi
  done < Cargo.toml
} || true)"

[ -n "$WORKSPACE_VERSION" ] || {
  echo "could not read [workspace.package].version from Cargo.toml" >&2
  exit 1
}
[ "$TAG_VERSION" = "$WORKSPACE_VERSION" ] || {
  echo "tag version $TAG_VERSION does not match workspace version $WORKSPACE_VERSION" >&2
  exit 1
}

CHANGELOG_MATCH=0
RELEASE_HEADING="## [$TAG_VERSION]"
DATED_HEADING_PREFIX="$RELEASE_HEADING - "
while IFS= read -r line; do
  if [ "$line" = "$RELEASE_HEADING" ]; then
    CHANGELOG_MATCH=1
    break
  fi
  if [[ "$line" == "$DATED_HEADING_PREFIX"* ]]; then
    release_date="${line#"$DATED_HEADING_PREFIX"}"
    if [[ "$release_date" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
      CHANGELOG_MATCH=1
      break
    fi
  fi
done < CHANGELOG.md
[ "$CHANGELOG_MATCH" -eq 1 ] || {
  echo "CHANGELOG.md has no released ## [$TAG_VERSION] section" >&2
  exit 1
}

git rev-parse --verify "${COMMIT}^{commit}" >/dev/null
git rev-parse --verify "${MAIN_REF}^{commit}" >/dev/null
git merge-base --is-ancestor "$COMMIT" "$MAIN_REF" || {
  echo "tagged commit $COMMIT is not reachable from $MAIN_REF" >&2
  exit 1
}

echo "PASS: release preflight ($TAG, $COMMIT, $MAIN_REF)"
