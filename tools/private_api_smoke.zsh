#!/bin/zsh
set -eu

if (( $# != 1 )); then
    print -u2 "usage: tools/private_api_smoke.zsh <board-ip-or-host>"
    exit 1
fi

script_dir="${0:A:h}"
repo_dir="${script_dir:h}"
smoke_slot="${PHYLLO_SMOKE_SLOT:-99}"
base_url="http://${1}"

if [[ "$smoke_slot" != <-> ]] || (( smoke_slot < 0 || smoke_slot >= 100 )); then
    print -u2 "PHYLLO_SMOKE_SLOT must be between 0 and 99"
    exit 1
fi

curl_api() {
    command curl --noproxy '*' --connect-timeout 10 "$@"
}

private_tmp="$(mktemp -d "${TMPDIR:-/tmp}/phyllo-api-smoke.XXXXXX")"
uploaded=0
ran=0
cleanup() {
    if (( ran )); then
        curl_api -sS --max-time 10 -o /dev/null \
            -X POST "$base_url/api/select/0" || true
    fi
    if (( uploaded )); then
        curl_api -sS --max-time 10 -o /dev/null \
            -X DELETE "$base_url/api/sketches/$smoke_slot" || true
    fi
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

wait_for_board() {
    local attempt
    for attempt in {1..15}; do
        if curl_api -sS --max-time 5 -o /dev/null "$base_url/api/health" 2>/dev/null; then
            return 0
        fi
        sleep 2
    done
    print -u2 "board did not recover within the upload timeout"
    return 1
}

wait_for_board
http_status="$(curl_api -sS --max-time 10 -o "$private_tmp/health.json" -w '%{http_code}' \
    "$base_url/api/health")"
expect_status health 200 "$http_status"

http_status="$(curl_api -sS --max-time 10 -o "$private_tmp/list.json" -w '%{http_code}' \
    "$base_url/api/sketches")"
expect_status list 200 "$http_status"
if grep -q "\"slot\":${smoke_slot}," "$private_tmp/list.json"; then
    print -u2 "slot $smoke_slot is occupied; refusing to overwrite it"
    exit 1
fi

print -n 'not a wasm module' >| "$private_tmp/invalid.wasm"
http_status="$(curl_api -sS --max-time 30 -o /dev/null -w '%{http_code}' \
    -X POST -H 'Content-Type: application/wasm' \
    --data-binary "@$private_tmp/invalid.wasm" \
    "$base_url/api/sketches/$smoke_slot/invalid-smoke")"
expect_status invalid-module 422 "$http_status"

dd if=/dev/zero of="$private_tmp/oversize.wasm" bs=32705 count=1 2>/dev/null
http_status="$(curl_api -sS --max-time 30 -o /dev/null -w '%{http_code}' \
    -X POST -H 'Content-Type: application/wasm' \
    --data-binary "@$private_tmp/oversize.wasm" \
    "$base_url/api/sketches/$smoke_slot/oversize-smoke")"
expect_status oversized-module 413 "$http_status"

http_status="$(curl_api -sS --max-time 30 -o /dev/null -w '%{http_code}' \
    -X POST -H 'Content-Type: application/wasm' \
    -H 'X-Phyllo-Color: #be28ff' \
    --data-binary "@$repo_dir/tools/testmod/testmod.wasm" \
    "$base_url/api/sketches/$smoke_slot/api-smoke")"
expect_status upload 201 "$http_status"
uploaded=1
wait_for_board

http_status="$(curl_api -sS --max-time 10 -o "$private_tmp/list-after.json" -w '%{http_code}' \
    "$base_url/api/sketches")"
expect_status list-after-upload 200 "$http_status"
grep -q "\"slot\":${smoke_slot},\"name\":\"api-smoke\"" "$private_tmp/list-after.json" || {
    print -u2 "uploaded sketch missing from listing"
    exit 1
}

http_status="$(curl_api -sS --max-time 30 -o /dev/null -w '%{http_code}' \
    -X POST "$base_url/api/run/$smoke_slot")"
expect_status run 202 "$http_status"
ran=1

sleep 2
http_status="$(curl_api -sS --max-time 10 -o /dev/null -w '%{http_code}' \
    -X POST "$base_url/api/select/0")"
expect_status return-to-main 202 "$http_status"
ran=0

http_status="$(curl_api -sS --max-time 10 -o /dev/null -w '%{http_code}' \
    -X DELETE "$base_url/api/sketches/$smoke_slot")"
expect_status delete 200 "$http_status"
uploaded=0

print "Public API smoke test passed; slot $smoke_slot is empty."
