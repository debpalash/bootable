#!/usr/bin/env bash
# Render the package-manager manifest templates under packaging/{winget,scoop,homebrew,flatpak,
# aur,nix} for one released Bootable version.
#
#   render-package-manifests.sh <version> [--sums FILE] [--out DIR] [--date YYYY-MM-DD]
#   render-package-manifests.sh --self-test
#
# Checksums come from, in order of preference:
#   1. --sums FILE   (lines of "<sha256>  <asset name>", i.e. a SHA256SUMS file or the
#                     concatenated per-asset .sha256 sidecars)
#   2. the release's SHA256SUMS asset, if one exists
#   3. the per-asset "<asset>.sha256" sidecars that the Release workflow publishes
# Nothing is pushed anywhere; the script only reads the public release and writes into --out.
set -euo pipefail

REPOSITORY="${BOOTABLE_REPOSITORY:-debpalash/bootable}"
ROOT="$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
ECOSYSTEMS=(winget scoop homebrew flatpak aur nix)
# KEY|asset-name pattern (VERSION is substituted). KEY becomes @SHA256_<KEY>@.
ASSETS=(
  'LINUX_APPIMAGE|bootable-VERSION-x86_64.AppImage'
  'LINUX_DEB|bootable_VERSION_amd64.deb'
  'LINUX_RPM|bootable-VERSION-1.x86_64.rpm'
  'LINUX_TARBALL|bootable-VERSION-x86_64-unknown-linux-gnu.tar.gz'
  'MACOS_DMG|bootable-VERSION-aarch64.dmg'
  'MACOS_TARBALL|bootable-VERSION-aarch64-apple-darwin.tar.gz'
  'WINDOWS_MSI|bootable-VERSION-x86_64.msi'
  'WINDOWS_SETUP|bootable-VERSION-x86_64-setup.exe'
  'WINDOWS_ZIP|bootable-VERSION-x86_64-pc-windows-msvc.zip'
  'WINDOWS_DESKTOP_EXE|bootable-desktop-VERSION-x86_64.exe'
  'WINDOWS_TUI_EXE|bootable-tui-VERSION-x86_64.exe'
)
PLACEHOLDER_PATTERN='@[A-Z][A-Z0-9_]*@'
BASE_TMP=""

die() {
  echo "render-package-manifests: $*" >&2
  exit 1
}

usage() {
  sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

fetch() {
  curl -fsSL --proto '=https' --tlsv1.2 --retry 3 --connect-timeout 20 --max-time 120 "$1"
}

asset_name() { # <pattern> <version>
  printf '%s' "${1//VERSION/$2}"
}

# Print the lowercase sha256 recorded for an asset in a normalized sums file, or nothing.
lookup_hash() { # <sums-file> <asset>
  awk -v asset="$2" '
    { name = $2; sub(/^\*/, "", name) }
    name == asset { print tolower($1) }
  ' "$1" | sort -u
}

normalize_sums() { # <input> <output>
  # Accept CRLF, a missing final newline, and the "*name" binary-mode marker.
  tr -d '\r' <"$1" | awk 'NF >= 2 { print }' >"$2"
}

release_date() { # <version> <explicit-date>
  local candidate="$2"
  if [ -z "$candidate" ] && [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
    candidate="$(date -u -d "@${SOURCE_DATE_EPOCH}" +%F 2>/dev/null || date -u -r "${SOURCE_DATE_EPOCH}" +%F)"
  fi
  if [ -z "$candidate" ]; then
    local api="https://api.github.com/repos/${REPOSITORY}/releases/tags/v$1"
    candidate="$(fetch "$api" 2>/dev/null | sed -n 's/.*"published_at": *"\([0-9-]\{10\}\)T.*/\1/p' | head -1 || true)"
  fi
  [[ "$candidate" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] ||
    die "could not determine the release date; pass --date YYYY-MM-DD"
  printf '%s' "$candidate"
}

render() { # <version> <sums-file|""> <out-dir> <date> <templates-dir>
  local version="$1" sums_arg="$2" out="$3" date_arg="$4" templates="$5"
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
    die "version must be a stable MAJOR.MINOR.PATCH (got '$version'); release candidates are not packaged"
  case "$out" in '' | /) die "refusing to use '$out' as the output directory" ;; esac

  local work entry key pattern asset found count
  work="$(mktemp -d "$BASE_TMP/render.XXXXXX")"

  local sums="$work/sums"
  : >"$sums"
  if [ -n "$sums_arg" ]; then
    [ -r "$sums_arg" ] || die "cannot read checksum file: $sums_arg"
    normalize_sums "$sums_arg" "$sums"
  else
    local base="https://github.com/${REPOSITORY}/releases/download/v${version}"
    local raw="$work/sums.raw"
    if fetch "$base/SHA256SUMS" >"$raw" 2>/dev/null; then
      normalize_sums "$raw" "$sums"
    fi
    for entry in "${ASSETS[@]}"; do
      pattern="${entry#*|}"
      asset="$(asset_name "$pattern" "$version")"
      if [ -z "$(lookup_hash "$sums" "$asset")" ]; then
        if fetch "$base/$asset.sha256" >"$raw" 2>/dev/null; then
          { tr -d '\r' <"$raw"; echo; } >>"$sums"
        fi
      fi
    done
    sed -i.bak '/^[[:space:]]*$/d' "$sums" && rm -f "$sums.bak"
  fi

  # Resolve every required checksum; report all problems at once.
  local sed_script="$work/substitute.sed" missing=() problems=()
  : >"$sed_script"
  for entry in "${ASSETS[@]}"; do
    key="${entry%%|*}"
    pattern="${entry#*|}"
    asset="$(asset_name "$pattern" "$version")"
    found="$(lookup_hash "$sums" "$asset")"
    count="$(printf '%s' "$found" | grep -c . || true)"
    if [ "$count" -eq 0 ]; then
      missing+=("$asset")
    elif [ "$count" -gt 1 ]; then
      problems+=("conflicting checksums for $asset")
    elif ! [[ "$found" =~ ^[0-9a-f]{64}$ ]]; then
      problems+=("malformed checksum for $asset: '$found'")
    else
      printf 's|@SHA256_%s@|%s|g\n' "$key" "$found" >>"$sed_script"
      printf 's|@SHA256_%s_UPPER@|%s|g\n' "$key" "$(printf '%s' "$found" | tr 'a-f' 'A-F')" >>"$sed_script"
    fi
  done
  if [ "${#missing[@]}" -gt 0 ]; then
    problems+=("no checksum for: ${missing[*]}")
  fi
  if [ "${#problems[@]}" -gt 0 ]; then
    printf 'render-package-manifests: %s\n' "${problems[@]}" >&2
    exit 1
  fi
  printf 's|@VERSION@|%s|g\n' "$version" >>"$sed_script"
  printf 's|@RELEASE_DATE@|%s|g\n' "$(release_date "$version" "$date_arg")" >>"$sed_script"

  # Render into a staging tree first so a failure never leaves a half-written output directory.
  local stage="$work/stage" eco source relative target rendered=0
  mkdir -p "$stage"
  for eco in "${ECOSYSTEMS[@]}"; do
    [ -d "$templates/$eco" ] || die "missing template directory: $templates/$eco"
    while IFS= read -r -d '' source; do
      relative="${source#"$templates"/}"
      relative="${relative%.in}"
      relative="${relative//@VERSION@/$version}"
      target="$stage/$relative"
      mkdir -p "$(dirname -- "$target")"
      sed -f "$sed_script" "$source" >"$target"
      rendered=$((rendered + 1))
    done < <(find "$templates/$eco" -type f -print0 | sort -z)
  done
  [ "$rendered" -gt 0 ] || die "no templates found under $templates"

  local leftovers
  leftovers="$(grep -rnE "$PLACEHOLDER_PATTERN" "$stage" || true)"
  if [ -n "$leftovers" ]; then
    printf 'render-package-manifests: unresolved placeholders remain:\n%s\n' "${leftovers//$stage\//}" >&2
    exit 1
  fi

  # Record exactly which checksums were used, then publish the staged tree idempotently.
  for entry in "${ASSETS[@]}"; do
    asset="$(asset_name "${entry#*|}" "$version")"
    printf '%s  %s\n' "$(lookup_hash "$sums" "$asset")" "$asset"
  done >"$stage/SHA256SUMS.used"

  mkdir -p "$out"
  for eco in "${ECOSYSTEMS[@]}"; do
    rm -rf "${out:?}/$eco"
    mv "$stage/$eco" "$out/$eco"
  done
  mv -f "$stage/SHA256SUMS.used" "$out/SHA256SUMS.used"
  echo "Rendered $rendered files for Bootable $version into $out"
}

validate_syntax() { # <rendered-dir>; best effort, skips tools that are not installed
  local out="$1" file checked=0
  if command -v ruby >/dev/null 2>&1; then
    while IFS= read -r -d '' file; do
      ruby -ryaml -rdate -e 'YAML.safe_load(File.read(ARGV[0]), permitted_classes: [Date])' "$file" ||
        die "invalid YAML: $file"
      checked=$((checked + 1))
    done < <(find "$out" -name '*.yaml' -o -name '*.yml' | sort | tr '\n' '\0')
    while IFS= read -r -d '' file; do
      ruby -rjson -e 'JSON.parse(File.read(ARGV[0]))' "$file" || die "invalid JSON: $file"
      checked=$((checked + 1))
    done < <(find "$out" -name '*.json' | sort | tr '\n' '\0')
    while IFS= read -r -d '' file; do
      ruby -c "$file" >/dev/null || die "invalid Ruby: $file"
      checked=$((checked + 1))
    done < <(find "$out" -name '*.rb' | sort | tr '\n' '\0')
  fi
  while IFS= read -r -d '' file; do
    if command -v xmllint >/dev/null 2>&1; then
      xmllint --noout "$file" || die "invalid XML: $file"
      checked=$((checked + 1))
    elif command -v python3 >/dev/null 2>&1; then
      python3 -c 'import sys, xml.etree.ElementTree as E; E.parse(sys.argv[1])' "$file" ||
        die "invalid XML: $file"
      checked=$((checked + 1))
    fi
  done < <(find "$out" -name '*.xml' | sort | tr '\n' '\0')
  while IFS= read -r -d '' file; do
    bash -n "$file" || die "invalid shell syntax: $file"
    checked=$((checked + 1))
  done < <(find "$out" -name PKGBUILD | sort | tr '\n' '\0')
  echo "Syntax-checked $checked rendered files"
}

self_test() {
  local tmp version=9.9.9 asset entry
  tmp="$(mktemp -d "$BASE_TMP/selftest.XXXXXX")"

  hash_of() { printf '%s' "$1" | { sha256sum 2>/dev/null || shasum -a 256; } | awk '{print $1}'; }

  # Fake release checksums: deterministic and distinct per asset.
  for entry in "${ASSETS[@]}"; do
    asset="$(asset_name "${entry#*|}" "$version")"
    printf '%s  %s\n' "$(hash_of "$asset")" "$asset"
  done >"$tmp/SHA256SUMS"

  echo "self-test: render succeeds and leaves no placeholders"
  render "$version" "$tmp/SHA256SUMS" "$tmp/out1" 2030-01-02 "$ROOT/packaging"
  validate_syntax "$tmp/out1"
  ! grep -rqE "$PLACEHOLDER_PATTERN" "$tmp/out1" || die "self-test: placeholder left behind"
  grep -rq "$(hash_of "$(asset_name 'bootable-VERSION-x86_64-setup.exe' "$version")" | tr 'a-f' 'A-F')" \
    "$tmp/out1/winget" || die "self-test: winget checksum missing"
  grep -rq "$(hash_of "$(asset_name 'bootable-VERSION-aarch64.dmg' "$version")")" \
    "$tmp/out1/homebrew" || die "self-test: cask checksum missing"
  [ -f "$tmp/out1/winget/manifests/d/debpalash/Bootable/$version/debpalash.Bootable.installer.yaml" ] ||
    die "self-test: winget manifest path not rendered"
  grep -q "^pkgver=$version" "$tmp/out1/aur/bootable-bin/PKGBUILD" || die "self-test: PKGBUILD version"

  echo "self-test: rendering twice into the same directory is idempotent"
  render "$version" "$tmp/SHA256SUMS" "$tmp/out2" 2030-01-02 "$ROOT/packaging"
  render "$version" "$tmp/SHA256SUMS" "$tmp/out2" 2030-01-02 "$ROOT/packaging"
  diff -r "$tmp/out1" "$tmp/out2" || die "self-test: output differs between runs"

  echo "self-test: CRLF sidecar-style input and the '*name' marker are accepted"
  sed -e 's/  /  */' -e 's/$/\r/' "$tmp/SHA256SUMS" >"$tmp/crlf"
  render "$version" "$tmp/crlf" "$tmp/out3" 2030-01-02 "$ROOT/packaging"
  diff -r "$tmp/out1" "$tmp/out3" || die "self-test: CRLF input rendered differently"

  echo "self-test: a missing checksum fails loudly"
  grep -v 'setup.exe' "$tmp/SHA256SUMS" >"$tmp/short"
  if (render "$version" "$tmp/short" "$tmp/out4" 2030-01-02 "$ROOT/packaging") 2>"$tmp/err"; then
    die "self-test: render accepted a missing checksum"
  fi
  grep -q 'no checksum for: .*setup.exe' "$tmp/err" || die "self-test: wrong missing-checksum message"
  [ ! -e "$tmp/out4" ] || die "self-test: failed render left output behind"

  echo "self-test: a malformed or conflicting checksum fails"
  sed 's/^[0-9a-f]\{64\}/deadbeef/' "$tmp/SHA256SUMS" >"$tmp/bad"
  if (render "$version" "$tmp/bad" "$tmp/out5" 2030-01-02 "$ROOT/packaging") 2>/dev/null; then
    die "self-test: render accepted a malformed checksum"
  fi
  { cat "$tmp/SHA256SUMS"; printf '%064d  %s\n' 0 "$(asset_name 'bootable-VERSION-aarch64.dmg' "$version")"; } >"$tmp/conflict"
  if (render "$version" "$tmp/conflict" "$tmp/out6" 2030-01-02 "$ROOT/packaging") 2>/dev/null; then
    die "self-test: render accepted conflicting checksums"
  fi

  echo "self-test: an unknown placeholder in a template fails"
  mkdir -p "$tmp/templates"
  for entry in "${ECOSYSTEMS[@]}"; do cp -R "$ROOT/packaging/$entry" "$tmp/templates/$entry"; done
  echo 'oops: @NOT_A_REAL_PLACEHOLDER@' >"$tmp/templates/scoop/extra.txt.in"
  if (render "$version" "$tmp/SHA256SUMS" "$tmp/out7" 2030-01-02 "$tmp/templates") 2>"$tmp/err"; then
    die "self-test: render accepted an unresolved placeholder"
  fi
  grep -q 'NOT_A_REAL_PLACEHOLDER' "$tmp/err" || die "self-test: placeholder not reported"

  echo "self-test: release candidates and bad versions are rejected"
  for bad in 1.2 1.2.3-rc.pr4.1 v1.2.3 ''; do
    if (render "$bad" "$tmp/SHA256SUMS" "$tmp/out8" 2030-01-02 "$ROOT/packaging") 2>/dev/null; then
      die "self-test: accepted version '$bad'"
    fi
  done

  echo "self-test: passed"
}

main() {
  local version="" sums="" out="$ROOT/dist/package-manifests" date_arg="" self=0
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --self-test) self=1 ;;
      --sums) sums="${2:?--sums needs a file}"; shift ;;
      --out) out="${2:?--out needs a directory}"; shift ;;
      --date) date_arg="${2:?--date needs YYYY-MM-DD}"; shift ;;
      -h|--help) usage ;;
      -*) echo "unknown option: $1" >&2; usage ;;
      *)
        [ -z "$version" ] || usage
        version="$1"
        ;;
    esac
    shift
  done

  BASE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/bootable-manifests.XXXXXX")"
  trap 'rm -rf "$BASE_TMP"' EXIT
  if [ "$self" -eq 1 ]; then
    [ -z "$version" ] || usage
    self_test
    return
  fi
  [ -n "$version" ] || usage
  render "$version" "$sums" "$out" "$date_arg" "$ROOT/packaging"
  validate_syntax "$out"
}

main "$@"
