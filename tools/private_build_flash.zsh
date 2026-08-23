#!/bin/zsh
set -eu

script_dir="${0:A:h}"
repo_dir="${script_dir:h}"
config_file="${PHYLLO_PRIVATE_ENV_FILE:-${XDG_CONFIG_HOME:-${HOME}/.config}/phyllotaxis/device.zsh}"

if [[ ! -r "$config_file" ]]; then
    print -u2 "Private configuration not found. Run tools/configure_private_env.zsh first."
    exit 1
fi

source "$config_file"
: "${PHYLLO_SSID:?PHYLLO_SSID is missing}"
: "${PHYLLO_PASS:=}"
: "${PHYLLO_API_TOKEN:?PHYLLO_API_TOKEN is missing}"

if (( ${#PHYLLO_API_TOKEN} < 16 )); then
    print -u2 "PHYLLO_API_TOKEN must contain at least 16 characters"
    exit 1
fi

private_target="$(mktemp -d "${TMPDIR:-/tmp}/phyllo-private-build.XXXXXX")"
trap '[[ -n "${private_target:-}" && -d "$private_target" ]] && rm -rf -- "$private_target"' EXIT

cd "$repo_dir"
CARGO_TARGET_DIR="$private_target" cargo build --release
espflash flash \
    --chip esp32s3 \
    --partition-table partitions.csv \
    "$private_target/xtensa-esp32s3-none-elf/release/esp32_phyllotaxis"

unset PHYLLO_SSID PHYLLO_PASS PHYLLO_API_TOKEN
print "Private build flashed; temporary build artifacts removed."
