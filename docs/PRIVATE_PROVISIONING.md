# Private network provisioning

Credentials must not be pasted into chat, command arguments, source files, or
the repository. Configure them once from a local terminal:

```sh
cd /Users/jagi/Programming/phyllotaxis/esp32_phyllotaxis
tools/configure_private_env.zsh
```

The script prompts without echo and stores shell-quoted exports in
`~/.config/phyllotaxis/device.zsh` with mode `600`. Set
`PHYLLO_PRIVATE_ENV_FILE` before running the script to choose another location.
The file is outside the repository.

Build and flash without leaving a credential-bearing firmware image in the
repository's `target/` directory:

```sh
tools/private_build_flash.zsh
```

The build helper sources the private file, builds in a fresh temporary
directory, flashes the board, unsets the variables, and removes the temporary
artifacts. It never prints credential values. Serial logs report only
`wifi connected`, never the SSID.

Pass `--monitor` to retain the temporary symbol file only for the duration of
a sanitized serial-monitor session. Exiting the monitor deletes it:

```sh
tools/private_build_flash.zsh --monitor
```

After the board reports its DHCP address, it also advertises
`phyllotaxis.local` over mDNS. On networks that allow multicast, either the
hostname or numeric address can be used:

```sh
tools/private_api_smoke.zsh phyllotaxis.local
```

It uses slot 99 only after confirming that the slot is empty. It tests reads,
upload bounds and validation, a valid upload/run, return to the main sketch,
and deletion. Set `PHYLLO_SMOKE_SLOT` to reserve a different empty test slot.

Shell exports created in one terminal cannot change an already-running Codex
process. The private file is the handoff mechanism for later automated tests:
approved helper commands can source it internally without displaying it. Do
not ask debugging commands to print the environment, enable shell tracing, or
run `strings` against the private firmware artifact.

The Wi-Fi credentials are compiled into the flashed firmware and anyone with
physical flash-read access can recover them. Sketch-management endpoints are
intentionally public to the local network; do not expose the board directly to
the public Internet.
