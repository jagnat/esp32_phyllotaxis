#!/bin/zsh
set -eu

config_dir="${XDG_CONFIG_HOME:-${HOME}/.config}/phyllotaxis"
config_file="${PHYLLO_PRIVATE_ENV_FILE:-${config_dir}/device.zsh}"

umask 077
mkdir -p -- "${config_file:h}"
chmod 700 -- "${config_file:h}"

read -s "ssid?Wi-Fi SSID (hidden): "
print
read -s "password?Wi-Fi password (hidden): "
print
read -s "api_token?API token, at least 16 characters (hidden): "
print

if (( ${#ssid} == 0 )); then
    print -u2 "SSID cannot be empty"
    exit 1
fi
if (( ${#api_token} < 16 )); then
    print -u2 "API token must contain at least 16 characters"
    exit 1
fi

private_tmp="$(mktemp "${config_file}.XXXXXX")"
trap '[[ -n "${private_tmp:-}" && -f "$private_tmp" ]] && rm -f -- "$private_tmp"' EXIT

{
    print -r -- '# Private phyllotaxis device configuration. Do not commit or print.'
    print -r -- "export PHYLLO_SSID=${(q)ssid}"
    print -r -- "export PHYLLO_PASS=${(q)password}"
    print -r -- "export PHYLLO_API_TOKEN=${(q)api_token}"
} >| "$private_tmp"

chmod 600 -- "$private_tmp"
mv -f -- "$private_tmp" "$config_file"
private_tmp=""
unset ssid password api_token

print "Private configuration saved to $config_file"
print "Values were not printed and are outside the repository."
