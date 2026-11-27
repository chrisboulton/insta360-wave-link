Your project is impressive, but I believe there's one important aspect that may currently be limiting its overall quality and potential.  I'd really like to discuss this with you in detail before moving forward with a pull request. I see an opportunity to introduce some meaningful improvements that could take the project to the next level and make it even more powerful.  I think there's potential here for something truly exceptional, and I'd love to share my thoughts with you.
# Insta360 Wave CLI

Control an Insta360 Wave over USB from macOS or Linux. This Rust CLI supports
audio, Wi-Fi, meetings, system settings, paired-camera controls, device account
operations, and staged firmware updates. The Rust library exposes the synchronous
USB client and packed reply decoders for a future UI.

## Install and run

Requires a Rust toolchain and a USB data cable.

```sh
cargo install --path . --locked
wave devices
wave info

# Retrieve the existing binding key once, capturing it into this shell's environment.
export WAVE_BIND_KEY="$(wave binding-key --controller-login)"

# Subsequent controls use only USB.
wave status
wave mic mute on
wave mic mute off
wave mic volume 80
wave speaker volume 60
wave speaker mute on
wave speaker mute off
wave wifi status
wave wifi list
wave system info
wave meeting status
wave ota status
```

Without installing, use `cargo run -- <arguments>`, for example
`cargo run -- --json status`. Use `cargo run --quiet -- binding-key --controller-login`
when capturing the key. Volume is an integer from 0 to 100; mute takes `on` or
`off`. Use `--json` for compact machine-readable output, `--serial <USB-serial>`
to select between multiple Waves, and `--timeout 10` to adjust the USB reply
timeout in seconds. Global options work before or after the command.

On Linux, the default HIDAPI backend needs a C compiler, `pkg-config`, and libudev
development headers (for example, `libudev-dev` on Debian/Ubuntu). macOS uses the
native HID backend built with the Xcode command-line tools.

## Account-bound devices

`devices`, `info`, and `camera status` work without credentials. An account-bound Wave encrypts
status/audio commands with the first 16 bytes of its existing binding key.

`wave binding-key --controller-login` reads the existing login from the installed
**Insta360 Wave Controller**'s `startup.ini` and retrieves the Wave's existing
binding key through a read-only HTTPS lookup. It requires a connected, bound
Wave, internet access, and a valid signed-in controller account.

**`binding-key` writes only the raw key and a newline to stdout**, including with
`--json`. Capture it as shown above; running it directly displays the secret.
Errors go to stderr with a nonzero exit status. It never prints the login token,
saves credentials, changes controller configuration, or binds/unbinds the Wave.
The lookup refuses redirects and accepts only supported Insta360 HTTPS endpoints.

The default configuration search supports the current and older macOS Wave
Controller locations. On other systems, provide its configuration explicitly:

```sh
export WAVE_BIND_KEY="$(wave binding-key --controller-login --controller-config /path/to/startup.ini)"
```

Alternatively, supply an existing Insta360 login token through `WAVE_LOGIN_TOKEN`
and run `wave binding-key`. This uses the US endpoint by default; select another
supported regional summary-service URL with `binding-key --endpoint <URL>`.
This is token reuse, not email/password sign-in. `--controller-login` selects the
controller's token and endpoint instead. Key retrieval always fetches the existing
key even if `WAVE_BIND_KEY` is already set.

All USB command groups consume only the supplied device credentials and never
perform a cloud lookup or read the controller's login. They work offline.
If a bound Wave has no supplied key, encrypted commands fail with retrieval instructions.
Unbound devices work without a key. Keep keys/tokens out of source files and shell
history. Run `unset WAVE_BIND_KEY WAVE_LOGIN_TOKEN` to clear them from your shell.

## Commands

Run `wave <group> --help` for arguments and `wave <group> <command> --help` for
details. Numeric enum values are retained where labels have not been established.

| Group | Commands |
|---|---|
| `mic`, `speaker` | `volume`, `mute`, `mode` |
| `wifi` | `scan`, `list`, `status`, `info`, `connect`, `disconnect`, `forget`, `enable`, `auto-connect` |
| `meeting` | `start`, `stop`, `pause`, `resume`, `status`, `info`, `current`, `mark`, `mode`, `screenshot`, `pc-status` |
| `system` | `info`, `settings`, `defaults`, `languages`, `countries`, `timezones`, `language`, `country`, `city`, `timezone`, `time`, `auto-time`, `time-format`, `datetime`, `storage`, `reboot`, `reset` |
| `camera` | `status`, `zoom`, `reset`, `move`, `stop`, `mode` |
| `account` | `info`, `bind-start`, `password`, `bind`, `auth`, `unauth`, `unbind`, `authorize-unbind`, `unbind-offline` |
| `ota` | `status`, `storage`, `start`, `finish` |
| `factory-mode` | `on`, `off` |

Examples (writes take effect immediately):

```sh
wave mic mode 0
wave wifi scan                 # Asynchronous; list after the scan completes.
wave wifi list
wave wifi connect "Example network" --encryption 3
wave wifi disconnect "Example network" --forget
wave wifi auto-connect on
wave system language          # Read; supplying a value sets it.
wave system country
wave system timezones --country "United States"
wave system time 2026-01-01T12:00:00
wave system auto-time on
wave system datetime 2026-01-01T12:00:00 --format 1 --auto off
wave meeting start
wave meeting mark 1
wave meeting pause
wave meeting resume
wave meeting stop
wave meeting info 1
wave camera zoom in
wave camera status            # Camera readback; no binding key required.
wave camera move 1
wave camera stop               # Equivalent to move 0.
wave camera mode 3            # Whiteboard mode.
wave account info             # Omits keys and tokens.
```

Set `WAVE_WIFI_PASSWORD` privately before `wifi connect`; an absent variable sends
an empty password. Use the encryption value returned by `wifi list`. Scan/connect
acknowledgements confirm acceptance; inspect subsequent results/status for completion.
The list can return firmware result 2 when results are unavailable. Timezone and
locale names should match the device catalog; clock values use device local time.
`meeting screenshot` registers an already-uploaded object name; it does not capture
or upload an image. `meeting pc-status` reports host application state to the Wave.

Move values are 0=stop, 1=pan -8, 2=pan +8, 3=tilt +8, 4=tilt -8.
Connect a compatible paired camera directly to the Wave. `camera status` requests
the Wave's short heartbeat over USB and reads connection, mode, zoom (100–400), and
privacy without a binding key. `status` uses this same readback for its camera fields.
Extended privacy is unavailable and reported as `null`; the similarly named
heartbeat byte actually contains a camera UUID color flag.

Modes are 0=off, 1=single tracking, 2=group tracking, 3=whiteboard, 4=desk view.
Movement continues until stopped; use `camera stop` when finished. Reset/movement
ACKs use 1 for success, indicating the Wave accepted the command. Zoom and mode
callbacks leave their ACK bytes uninitialized and report `verified: false`; use
`camera status` to confirm their effects. Position readback is not exposed by the
recovered PC-Link commands, so movement/reset cannot be verified automatically.
Camera writes check the heartbeat connection first and fail if no camera is attached.
The firmware's direct desk-view exit briefly reports off, then reverts to desk view
after synchronization. `camera mode 0` works around this by switching from desk view
through single tracking before turning it off. This briefly activates tracking.
The separate UART camera commands for absolute PTZ/zoom, privacy, and device info
are not forwarded by the Wave's ordinary USB PC-Link receiver. Cascade mode remains
unsupported.

### Device accounts

These are explicit USB firmware operations, separate from cloud sign-in and the
read-only `binding-key` lookup. They do not implement a complete cloud binding workflow.
`account password request` opens verification on the Wave's screen;
`account password poll` returns pending/verified/failed. The firmware ignores the
password string field; the CLI therefore sends zeros rather than accepting a password.
`account auth <user-id>` uses `WAVE_BIND_KEY`; `account authorize-unbind` uses
`WAVE_DEVICE_TOKEN`. Credentials are never included in decoded replies.

`account bind /private/path/binding.json` writes a complete 1216-byte device binding
record. Every field below is required; empty strings/zeroes must be intentional.
The file is read without modification, and parser errors never quote its contents.
On an already-bound device, `WAVE_BIND_KEY` still supplies the current encryption key.
The `key` in the record is the new binding key; later invocations must supply it.

| JSON fields | Type / packed width |
|---|---|
| `user_id`, `key`, `area`, `cloud_token` | strings: 16, 64, 128, 512 bytes |
| `nickname`, `email`, `phone`, `membership_url` | strings: 128, 128, 32, 128 bytes |
| `storage_limit`, `storage_used`, `vip_level` | strings: 16 bytes each |
| `benefit_reset_time`, `vip_expire_time` | unsigned 64-bit integers |
| `transcription_limit`, `transcription_used`, `llm_limit`, `llm_used` | unsigned 32-bit integers |

Keep this file outside the project. `account unbind --remove-user-data` can erase
device user data; offline unbinding invokes the firmware's separate offline handler.

### System and firmware operations

`system reset` restores factory settings; `system reboot` reboots the device.
`system storage on` and `ota storage` can disconnect the active HID interface as
USB modes change. `factory-mode on` persists factory mode and requests CDC/ADB USB;
turning it off requests the normal mode. A disconnect can precede an acknowledgement.

`ota start <package-filename>` starts installation of a package already present under
the Wave's `/mnt/udisk/misc/data/controlsystemfile/` directory. The CLI does not upload
packages, download firmware, or bypass the firmware's package/battery checks.
`ota finish <status>` acknowledges completion; a nonzero value requests leaving OTA
mode. Status zero clears local state but leaves the ACK byte undefined, so its output
also reports `verified: false`. Read `ota status` and `status` to inspect progress.

## USB behavior and limitations

Each invocation resets the HID transport queues to start a fresh packet stream.
This is a protocol reset, not a device reboot or factory reset. Close the official
controller while using the CLI to avoid competing USB clients. Disconnect a
paired Link camera if the Wave is in cascade mode.

The client validates MAVLink checksums, reassembles PC-Link replies, suppresses
received duplicates, requests missing HID reports, answers resend/sync requests,
and acknowledges short heartbeats. It waits for the transport-reset response,
splits requests into 112-byte packets, and supports replies up to 32 KiB. It never automatically repeats a logical
state-changing command after a timeout; read status before retrying a failed
write. A successful audio command means the Wave returned a success
acknowledgement. Read `status` to inspect the current value.

Numeric mode/status/error fields use the recovered firmware values; their labels
are not yet fully mapped. All 61 registered ordinary command handlers other than
bulk file transfer are exposed. File transfer (71) and out-of-band recovery (112/113)
still need a complete transfer/resumption implementation. IDs 44 (speaker-volume
query), 57 (timezone query), and 65/66 (ASR start/stop) appear in the enum but have no
registered handlers; use `status`/`system info` for the corresponding reads.

**Large catalog limitation:** the inspected firmware's HID send queue holds 400
reports and drops unsent reports when full. Encrypted country/all-settings replies
span approximately 447/687 reports; discarded reports cannot be retransmitted.
Live `system countries` and `system settings` failed with this condition. The smaller
`system timezones` query also encountered unrecoverable reply loss during validation;
its remaining cause has not been established. These commands expose the recovered
protocol, but their live results cannot currently be relied upon. The client reports
reply loss and never emits a partial catalog. Smaller locale reads work.
Linux may require a hidraw permission/udev rule for vendor `18f0`, product `4e40`.

## Validation

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

Tests cover wire layout, MAVLink corruption/truncation, AES compatibility, packet
assembly/retransmission, heartbeat handling, credential handling, and CLI
validation. Cloud request tests use a local server and synthetic credentials.
Live validation on macOS previously checked all four volume/mute controls with
readback and restoration. This expansion checked status, Wi-Fi info/status/list,
system info/defaults/language/country/city/languages, meeting status/current,
OTA status, and redacted account info. Tests also cover multi-packet requests,
large catalog assembly, calendar/field validation, and firmware ACK quirks.
Live paired-camera validation checked zoom 100→110→100 and modes 1–4→off against
short heartbeat readback. All four movement directions, stop, and position reset
returned successful ACKs. Zoom/mode were restored, movement stopped, and position
reset was sent; the previous position could not be read or restored. Regression
tests cover camera ACK semantics, heartbeat decoding, and undefined status fields.
An opt-in live regression test checks that desk-view exit remains off after six
seconds of camera synchronization:

```sh
# Requires a connected paired camera and WAVE_BIND_KEY; temporarily changes camera modes.
cargo test live_camera_can_exit_desk_view -- --ignored
```

Other new writes, account changes, factory reset, and OTA operations have not been
exercised on live hardware.
Linux hardware access has not yet been tested.

Protocol details and firmware evidence: [protocol/wave-usb.md](protocol/wave-usb.md).
