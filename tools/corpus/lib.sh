# Shared by fetch.sh and fetch-jsonld.sh (sourced, not run).

# One download: https only, redirects included; a time limit per file.
download() {
  curl -sSL --fail --proto '=https' --proto-redir '=https' --max-time 600 -A turtle-lsp-corpus "$@"
}

# sha256 of the file on stdin or at $1 (sha256sum on Linux and Git Bash, shasum
# on macOS).
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$@" | cut -d' ' -f1
  else
    shasum -a 256 "$@" | cut -d' ' -f1
  fi
}

# sha256 of a directory: the hash of one "HASH  ./path" line per file, sorted by
# path in the C locale.
sha256_tree() {
  local dir=$1 f
  (
    cd "$dir"
    find . -type f | LC_ALL=C sort | while IFS= read -r f; do
      printf '%s  %s\n' "$(sha256_of "$f")" "$f"
    done
  ) | sha256_of
}

changed=0
unchecked=0

# check_sum NAME PATH EXPECTED: warn when the file (or directory) at PATH does
# not hash to EXPECTED. An empty EXPECTED is not checked. The file stays.
check_sum() {
  local name=$1 path=$2 want=$3 got
  if [ -z "$want" ]; then
    unchecked=$((unchecked + 1))
    return 0
  fi
  if [ -d "$path" ]; then got=$(sha256_tree "$path"); else got=$(sha256_of "$path"); fi
  if [ "$got" != "$want" ]; then
    printf 'WARNING %s: sha256 changed\n  expected %s\n  got      %s\n' "$name" "$want" "$got" >&2
    changed=$((changed + 1))
  fi
  return 0
}

# checksum_summary: the checksum totals; status 1 for a change under
# CORPUS_STRICT=1, else 0.
checksum_summary() {
  printf 'checksums: %s changed, %s unchecked\n' "$changed" "$unchecked"
  if [ "$changed" -gt 0 ] && [ -n "${CORPUS_STRICT:-}" ]; then return 1; fi
  return 0
}
