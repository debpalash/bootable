#!/bin/sh
set -eu

usage() {
  echo "usage: qemu-uefi-smoke.sh [--cdrom|--disk|--usb] IMAGE [SCREENSHOT.png] [EXPECTED_RGB]" >&2
  echo "       EXPECTED_RGB is six hexadecimal digits; tolerance defaults to 48/channel" >&2
  exit 2
}

mode="${1:-}"
image="${2:-}"
screenshot="${3:-qemu-uefi-smoke.png}"
expected_rgb="${4:-}"
case "$mode" in
  --cdrom|--disk|--usb) ;;
  *) usage ;;
esac
[ -n "$image" ] || usage
[ -r "$image" ] || {
  echo "Image is not readable: $image" >&2
  exit 1
}
case "$expected_rgb" in
  ""|[0123456789abcdefABCDEF][0123456789abcdefABCDEF][0123456789abcdefABCDEF][0123456789abcdefABCDEF][0123456789abcdefABCDEF][0123456789abcdefABCDEF]) ;;
  *) usage ;;
esac

for command in qemu-system-x86_64 socat; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "Required command is missing: $command" >&2
    exit 1
  }
done

ovmf_code="${BOOTABLE_OVMF_CODE:-}"
ovmf_vars="${BOOTABLE_OVMF_VARS:-}"
if [ -z "$ovmf_code" ]; then
  for candidate in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/edk2/x64/OVMF_CODE.4m.fd; do
    if [ -r "$candidate" ]; then
      ovmf_code="$candidate"
      break
    fi
  done
fi
if [ -z "$ovmf_vars" ]; then
  for candidate in /usr/share/OVMF/OVMF_VARS_4M.fd /usr/share/edk2/x64/OVMF_VARS.4m.fd; do
    if [ -r "$candidate" ]; then
      ovmf_vars="$candidate"
      break
    fi
  done
fi
[ -r "$ovmf_code" ] && [ -r "$ovmf_vars" ] || {
  echo "OVMF firmware was not found; set BOOTABLE_OVMF_CODE and BOOTABLE_OVMF_VARS" >&2
  exit 1
}

work="$(mktemp -d "${TMPDIR:-/tmp}/bootable-qemu.XXXXXX")"
monitor="$work/monitor.sock"
variables="$work/OVMF_VARS.fd"
frame="$work/frame.ppm"
pidfile="$work/qemu.pid"
cp "$ovmf_vars" "$variables"

cleanup() {
  if [ -f "$pidfile" ]; then
    pid="$(cat "$pidfile")"
    kill "$pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT HUP INT TERM

set -- \
  -name bootable-uefi-smoke \
  -machine q35,accel=tcg \
  -cpu max \
  -smp 2 \
  -m "${BOOTABLE_QEMU_MEMORY:-2048}" \
  -drive "if=pflash,format=raw,readonly=on,file=$ovmf_code" \
  -drive "if=pflash,format=raw,file=$variables" \
  -boot menu=on,strict=on \
  -net none \
  -display none \
  -monitor "unix:$monitor,server=on,wait=off" \
  -daemonize \
  -pidfile "$pidfile"

if [ "$mode" = "--cdrom" ]; then
  set -- "$@" -drive "file=$image,media=cdrom,format=raw,readonly=on"
elif [ "$mode" = "--usb" ]; then
  set -- "$@" \
    -device qemu-xhci,id=bootable-xhci \
    -drive "if=none,id=bootable-usb,file=$image,format=raw,readonly=on" \
    -device usb-storage,drive=bootable-usb,removable=true,bootindex=1
else
  set -- "$@" -drive "file=$image,if=virtio,format=raw,readonly=on"
fi

qemu-system-x86_64 "$@"
if [ "$mode" = "--cdrom" ]; then
  # Windows optical media intentionally waits for a key before booting. Repeated
  # harmless spaces also cover slower TCG firmware startup.
  for _attempt in 1 2 3 4 5; do
    sleep 1
    printf 'sendkey spc\n' | socat - "UNIX-CONNECT:$monitor" >/dev/null
  done
fi
wait_seconds="${BOOTABLE_QEMU_WAIT_SECONDS:-20}"
sleep "$wait_seconds"
printf 'screendump %s\nquit\n' "$frame" | socat - "UNIX-CONNECT:$monitor" >/dev/null

mkdir -p "$(dirname "$screenshot")"
if command -v magick >/dev/null 2>&1; then
  magick "$frame" "$screenshot"
elif command -v convert >/dev/null 2>&1; then
  convert "$frame" "$screenshot"
elif command -v ffmpeg >/dev/null 2>&1; then
  ffmpeg -loglevel error -y -i "$frame" "$screenshot"
else
  cp "$frame" "${screenshot%.png}.ppm"
  screenshot="${screenshot%.png}.ppm"
fi

echo "UEFI smoke screenshot: $screenshot"

if [ -n "$expected_rgb" ]; then
  command -v ffmpeg >/dev/null 2>&1 || {
    echo "ffmpeg is required for the machine-readable color assertion" >&2
    exit 1
  }
  observed="$({
    ffmpeg -loglevel error -i "$frame" -vf scale=1:1 -frames:v 1 -f rawvideo -pix_fmt rgb24 -
  } | od -An -tu1 -N3)"
  # Deliberate word splitting turns the three byte values into positional parameters.
  set -- $observed
  [ "$#" -eq 3 ] || {
    echo "Could not sample the captured UEFI frame" >&2
    exit 1
  }
  observed_red="$1"
  observed_green="$2"
  observed_blue="$3"
  expected_red=$((0x$(printf '%s' "$expected_rgb" | cut -c1-2)))
  expected_green=$((0x$(printf '%s' "$expected_rgb" | cut -c3-4)))
  expected_blue=$((0x$(printf '%s' "$expected_rgb" | cut -c5-6)))
  tolerance="${BOOTABLE_QEMU_COLOR_TOLERANCE:-48}"
  case "$tolerance" in
    ""|*[!0-9]*)
      echo "BOOTABLE_QEMU_COLOR_TOLERANCE must be a non-negative integer" >&2
      exit 2
      ;;
  esac
  within_tolerance() {
    actual="$1"
    expected="$2"
    difference=$((actual - expected))
    [ "$difference" -lt 0 ] && difference=$((-difference))
    [ "$difference" -le "$tolerance" ]
  }
  if ! within_tolerance "$observed_red" "$expected_red" \
    || ! within_tolerance "$observed_green" "$expected_green" \
    || ! within_tolerance "$observed_blue" "$expected_blue"; then
    echo "UEFI frame assertion failed: observed RGB $observed_red,$observed_green,$observed_blue; expected #$expected_rgb ±$tolerance/channel" >&2
    exit 1
  fi
  echo "UEFI frame assertion passed: RGB $observed_red,$observed_green,$observed_blue matches #$expected_rgb ±$tolerance/channel"
fi
