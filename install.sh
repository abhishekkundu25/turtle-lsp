#!/bin/sh
# Installs the turtle-lsp binary from a GitHub release, on macOS and Linux:
#
#   curl -fsSL https://raw.githubusercontent.com/abhishekkundu25/turtle-lsp/main/install.sh | sh
#
# It downloads the release archive for this machine and SHA256SUMS from the
# same release, checks the archive against it, and puts turtle-lsp in the
# install folder. It asks nothing, changes no shell profile and no PATH, and
# prints the installed binary's path as its last line on stdout.
#
# Environment (set on `sh`: `curl -fsSL .../install.sh | TURTLE_LSP_VERSION=v0.19.0 sh`):
#   TURTLE_LSP_VERSION       a release tag such as v0.19.0 (default: the latest)
#   TURTLE_LSP_INSTALL_DIR   where turtle-lsp goes (default: ~/.local/bin)
#   TURTLE_LSP_VERIFY        "attestation": also check the archive's build
#                            attestation with `gh attestation verify` (needs gh,
#                            signed in); a failure stops the install
#   TURTLE_LSP_RELEASE_URL   the folder the files come from, instead of the
#                            GitHub release (tests and mirrors)
#
# Exit codes: 0 installed, 1 failed (the reason is on stderr).

set -eu

REPO=abhishekkundu25/turtle-lsp

say() { printf 'turtle-lsp install: %s\n' "$*" >&2; }
die() {
  say "error: $*"
  exit 1
}

# The release archive for this machine; the musl build runs on every Linux.
asset() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$os" in
    Darwin) os=darwin ;;
    Linux) os=linux ;;
    *) die "no release build for $os; on Windows run install.ps1 (see the README)" ;;
  esac
  case "$arch" in
    x86_64 | amd64) arch=x64 ;;
    arm64 | aarch64) arch=arm64 ;;
    *) die "no release build for $os on $arch; build it from source with cargo (see the README)" ;;
  esac
  echo "turtle-lsp-$os-$arch.tar.gz"
}

fetch() { # url file
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 2 -o "$2" "$1" </dev/null || die "could not download $1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1" </dev/null || die "could not download $1"
  else
    die "neither curl nor wget is installed"
  fi
}

sha256() { # file
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" </dev/null | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" </dev/null | cut -d' ' -f1
  else
    die "neither sha256sum nor shasum is installed, so the download cannot be checked"
  fi
}

main() {
  name=$(asset)
  version=${TURTLE_LSP_VERSION:-}
  if [ -n "${TURTLE_LSP_RELEASE_URL:-}" ]; then
    base=$TURTLE_LSP_RELEASE_URL
  elif [ -n "$version" ]; then
    base="https://github.com/$REPO/releases/download/$version"
  else
    base="https://github.com/$REPO/releases/latest/download"
  fi
  dir=${TURTLE_LSP_INSTALL_DIR:-$HOME/.local/bin}

  tmp=$(mktemp -d 2>/dev/null || mktemp -d -t turtle-lsp)
  trap 'rm -rf "$tmp"' EXIT INT TERM

  say "downloading $name from $base"
  fetch "$base/$name" "$tmp/$name"
  fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"
  # The line for this archive, by its exact name.
  want=$(awk -v n="$name" '$2 == n || $2 == "*" n { print $1 }' "$tmp/SHA256SUMS")
  [ -n "$want" ] || die "SHA256SUMS at $base has no line for $name"
  got=$(sha256 "$tmp/$name")
  [ "$got" = "$want" ] || die "$name does not match SHA256SUMS (got $got, expected $want)"

  if [ "${TURTLE_LSP_VERIFY:-}" = attestation ]; then
    command -v gh >/dev/null 2>&1 || die "TURTLE_LSP_VERIFY=attestation needs gh"
    gh attestation verify "$tmp/$name" --repo "$REPO" </dev/null >&2 ||
      die "gh attestation verify rejected $name (is gh signed in? gh auth login)"
    say "build attestation verified"
  fi

  mkdir -p "$tmp/x"
  tar -xzf "$tmp/$name" -C "$tmp/x" </dev/null || die "could not unpack $name"
  [ -f "$tmp/x/turtle-lsp" ] || die "$name has no turtle-lsp binary"
  chmod 755 "$tmp/x/turtle-lsp"
  # `--version` only: a bare turtle-lsp starts the language server.
  installed=$("$tmp/x/turtle-lsp" --version </dev/null) || die "the downloaded turtle-lsp does not run on this machine"

  mkdir -p "$dir" || die "cannot create $dir"
  # Renamed into place, never written over: a running copy keeps its file.
  cp "$tmp/x/turtle-lsp" "$dir/.turtle-lsp.new" || die "cannot write to $dir"
  mv -f "$dir/.turtle-lsp.new" "$dir/turtle-lsp" || die "cannot replace $dir/turtle-lsp"

  say "installed $installed"
  case ":$PATH:" in
    *":$dir:"*)
      first=$(command -v turtle-lsp 2>/dev/null || true)
      if [ -n "$first" ] && [ "$first" != "$dir/turtle-lsp" ]; then
        say "warning: $first comes first on PATH, not this one"
      fi
      ;;
    *) say "$dir is not on PATH: add it, or run $dir/turtle-lsp" ;;
  esac
  echo "$dir/turtle-lsp"
}

main "$@"
