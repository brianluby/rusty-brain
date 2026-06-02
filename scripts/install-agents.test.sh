#!/bin/sh
# install-agents.test.sh — POSIX-sh assertions for scripts/install-agents.sh.
# Sources the installer with INSTALL_AGENTS_SH_TESTING=1 so only functions are
# defined (main() is not run), then exercises the pure helpers against a
# scratch directory built with `mktemp -d`.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
INSTALL_AGENTS_SH_TESTING=1
export INSTALL_AGENTS_SH_TESTING
# shellcheck source=scripts/install-agents.sh
. "${HERE}/install-agents.sh"

fail() {
  printf 'TEST FAIL: %s\n' "$1" >&2
  exit 1
}

# --- sha256_of produces a stable hex digest -------------------------------
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
printf 'hello rusty-brain\n' > "${work}/a.bin"
printf 'hello rusty-brain\n' > "${work}/b.bin"
ha="$(sha256_of "${work}/a.bin")"
hb="$(sha256_of "${work}/b.bin")"
[ -n "$ha" ] || fail "sha256_of returned empty digest"
[ "$ha" = "$hb" ] || fail "identical files must hash identically ($ha vs $hb)"

# --- install_one copies, chmod +x, and checksum-verifies ------------------
src_dir="${work}/src"
dst_dir="${work}/bin"
mkdir -p "$src_dir" "$dst_dir"
printf '#!/bin/sh\necho hi\n' > "${src_dir}/rusty-brain-hooks"
chmod +x "${src_dir}/rusty-brain-hooks"

install_one "$src_dir" "$dst_dir" "rusty-brain-hooks"

[ -f "${dst_dir}/rusty-brain-hooks" ] || fail "binary was not copied to dst"
[ -x "${dst_dir}/rusty-brain-hooks" ] || fail "copied binary is not executable"
src_hash="$(sha256_of "${src_dir}/rusty-brain-hooks")"
dst_hash="$(sha256_of "${dst_dir}/rusty-brain-hooks")"
[ "$src_hash" = "$dst_hash" ] || fail "checksum mismatch after copy ($src_hash vs $dst_hash)"

# --- install_one fails loudly when the source binary is missing -----------
if install_one "$src_dir" "$dst_dir" "rusty-brain-install" 2>/dev/null; then
  fail "install_one must fail when the source binary is absent"
fi

printf 'TEST PASS: install-agents.sh helpers behave\n'
