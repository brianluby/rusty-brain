#!/bin/sh
# install-agents.sh — place the rusty-brain agent-surface binaries
# (rusty-brain-hooks, rusty-brain-install) alongside rusty-brain in
# ~/.local/bin, chmod +x, and SHA-256 verify each copy.
#
# Usage:
#   scripts/install-agents.sh [BUILD_DIR]
#
# BUILD_DIR defaults to "target/release" (relative to the repo root, or
# absolute). Override the install location with RUSTY_BRAIN_INSTALL_DIR.
#
# This script NEVER downloads anything and NEVER modifies shell config.
set -eu

# ---------- sha256_of --------------------------------------------------------
# Print the lowercase hex SHA-256 of "$1" using the first available tool.
sha256_of() {
  _file="${1:-}"
  if [ ! -f "$_file" ]; then
    printf 'ERROR: cannot hash missing file: %s\n' "$_file" >&2
    return 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$_file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$_file" | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$_file" | awk '{print $NF}'
  else
    printf 'ERROR: no SHA-256 tool found (need sha256sum, shasum, or openssl)\n' >&2
    return 1
  fi
}

# ---------- install_one ------------------------------------------------------
# Copy "$3" from src dir "$1" to dst dir "$2", chmod +x, checksum-verify.
install_one() {
  _src_dir="${1:-}"
  _dst_dir="${2:-}"
  _name="${3:-}"
  _src="${_src_dir}/${_name}"
  _dst="${_dst_dir}/${_name}"

  if [ ! -f "$_src" ]; then
    printf 'ERROR: source binary not found: %s\n' "$_src" >&2
    printf 'Build it first, e.g.: cargo build --release -p rb-hooks -p rb-install\n' >&2
    return 1
  fi

  mkdir -p "$_dst_dir"
  cp "$_src" "$_dst"
  chmod +x "$_dst"

  _src_hash="$(sha256_of "$_src")"
  _dst_hash="$(sha256_of "$_dst")"
  if [ "$_src_hash" != "$_dst_hash" ]; then
    printf 'ERROR: checksum mismatch after copying %s\n' "$_name" >&2
    printf '  source: %s\n' "$_src_hash" >&2
    printf '  copy:   %s\n' "$_dst_hash" >&2
    rm -f "$_dst"
    return 1
  fi

  printf 'Installed %s -> %s (sha256 %s)\n' "$_name" "$_dst" "$_src_hash"
}

# ---------- main -------------------------------------------------------------
main() {
  build_dir="${1:-target/release}"
  install_dir="${RUSTY_BRAIN_INSTALL_DIR:-$HOME/.local/bin}"

  if [ ! -d "$build_dir" ]; then
    printf 'ERROR: build dir does not exist: %s\n' "$build_dir" >&2
    printf 'Build the agent binaries first:\n' >&2
    printf '  cargo build --release -p rb-hooks -p rb-install\n' >&2
    return 1
  fi

  printf 'Installing agent binaries from %s to %s\n' "$build_dir" "$install_dir"
  install_one "$build_dir" "$install_dir" "rusty-brain-hooks"
  install_one "$build_dir" "$install_dir" "rusty-brain-install"

  # Informational PATH note only — never modify shell config.
  case ":${PATH}:" in
    *":${install_dir}:"*) ;;
    *)
      printf '\nNOTE: %s is not in your PATH.\n' "$install_dir"
      # shellcheck disable=SC2016
      printf '  export PATH="%s:$PATH"\n' "$install_dir"
      ;;
  esac

  printf '\nAgent binaries installed. Register hooks with:\n'
  printf '  rusty-brain-install install --agents claude-code\n'
}

# Test guard: when sourced by install-agents.test.sh, only define functions.
if [ "${INSTALL_AGENTS_SH_TESTING:-0}" != "1" ]; then
  main "$@"
fi
