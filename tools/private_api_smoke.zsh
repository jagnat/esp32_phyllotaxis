#!/bin/zsh
set -eu

if (( $# != 1 )); then
    print -u2 "usage: tools/private_api_smoke.zsh <board-ip-or-host>"
    exit 1
fi

script_dir="${0:A:h}"
repo_dir="${script_dir:h}"
config_file="${PHYLLO_PRIVATE_ENV_FILE:-${XDG_CONFIG_HOME:-${HOME}/.config}/phyllotaxis/device.zsh}"
smoke_slot="${PHYLLO_SMOKE_SLOT:-99}"
base_url="http://${1}"

if [[ ! -r "$config_file" ]]; then
    print -u2 "Private configuration not found. Run tools/configure_private_env.zsh first."
    exit 1
fi
if [[ "$smoke_slot" != <-> ]] || (( smoke_slot < 0 || smoke_slot >= 100 )); then
    print -u2 "PHYLLO_SMOKE_SLOT must be between 0 and 99"
    exit 1
fi

source "$config_file"
: "${PHYLLO_API_TOKEN:?PHYLLO_API_TOKEN is missing}"
auth_header="Authorization: Bearer ${PHYLLO_API_TOKEN}"

private_tmp="$(mktemp -d "${TMPDIR:-/tmp}/phyllo-api-smoke.XXXXXX")"
uploaded=0
ran=0
cleanup() {
    if (( ran )); then
        curl -sS --max-time 10 -o /dev/null \
            -X POST -H "$auth_header" "$base_url/api/select/0" || true
    fi
    if (( uploaded )); then
        curl -sS --max-time 10 -o /dev/null \
            -X DELETE -H "$auth_header" "$base_url/api/sketches/$smoke_slot" || true
    fi
    unset PHYLLO_SSID PHYLLO_PASS PHYLLO_API_TOKEN auth_header
    [[ -d "$private_tmp" ]] && rm -rf -- "$private_tmp"
}
trap cleanup EXIT

expect_status() {
    local label="$1"
    local expected="$2"
    local actual="$3"
    if [[ "$actual" != "$expected" ]]; then
        print -u2 "$label: expected HTTP $expected, got $actual"
        exit 1
    fi
    print "$label: HTTP $actual"
}

status="$(curl -sS --max-time 10 -o "$private_tmp/health.json" -w '%{http_code}' \
    "$base_url/api/health")"
expect_status health 200 "$status"

status="$(curl -sS --max-time 10 -o "$private_tmp/list.json" -w '%{http_code}' \
    "$base_url/api/sketches")"
expect_status list 200 "$status"
if grep -q "\"slot\":${smoke_slot}," "$private_tmp/list.json"; then
    print -u2 "slot $smoke_slot is occupied; refusing to overwrite it"
    exit 1
fi

status="$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' \
    -X DELETE "$base_url/api/sketches/$smoke_slot")"
expect_status unauthenticated-write 401 "$status"

print -n 'not a wasm module' >| "$private_tmp/invalid.wasm"
status="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' \
    -X POST -H "$auth_header" -H 'Content-Type: application/wasm' \
    --data-binary "@$private_tmp/invalid.wasm" \
    "$base_url/api/sketches/$smoke_slot/invalid-smoke")"
expect_status invalid-module 422 "$status"

dd if=/dev/zero of="$private_tmp/oversize.wasm" bs=32705 count=1 2>/dev/null
status="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' \
    -X POST -H "$auth_header" -H 'Content-Type: application/wasm' \
    --data-binary "@$private_tmp/oversize.wasm" \
    "$base_url/api/sketches/$smoke_slot/oversize-smoke")"
expect_status oversized-module 413 "$status"

status="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' \
    -X POST -H "$auth_header" -H 'Content-Type: application/wasm' \
    -H 'X-Phyllo-Color: #be28ff' \
    --data-binary "@$repo_dir/tools/testmod/testmod.wasm" \
    "$base_url/api/sketches/$smoke_slot/private-smoke")"
expect_status upload 201 "$status"
uploaded=1

status="$(curl -sS --max-time 10 -o "$private_tmp/list-after.json" -w '%{http_code}' \
    "$base_url/api/sketches")"
expect_status list-after-upload 200 "$status"
grep -q "\"slot\":${smoke_slot},\"name\":\"private-smoke\"" "$private_tmp/list-after.json" || {
    print -u2 "uploaded sketch missing from listing"
    exit 1
}

status="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' \
    -X POST -H "$auth_header" "$base_url/api/run/$smoke_slot")"
expect_status run 202 "$status"
ran=1

sleep 2
status="$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' \
    -X POST -H "$auth_header" "$base_url/api/select/0")"
expect_status return-to-main 202 "$status"
ran=0

status="$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' \
    -X DELETE -H "$auth_header" "$base_url/api/sketches/$smoke_slot")"
expect_status delete 200 "$status"
uploaded=0

print "Authenticated API smoke test passed; slot $smoke_slot is empty."
