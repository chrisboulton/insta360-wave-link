# Insta360 Wave USB protocol

This document describes the Wave's USB gadget configuration, HID transport, sequencing, and command families recovered from `libins_entry.so` and `S89usbgadget`.

## Source files

- `extracted/Wave/rootfs/etc/init.d/S89usbgadget`
- `extracted/Wave/rootfs/usr/lib/libins_entry.so`

## Device identity

The gadget configfs setup defines:

```text
idVendor:    0x18F0
idProduct:   0x4E40
manufacturer: insta360
product:      Insta360 Wave USB
serial:       PSP_<cpu-serial-suffix>
```

The composite configuration string varies by mode.

## Gadget modes

The startup script can construct these USB functions:

- `uac2.0` — USB Audio Class 2.0, 48 kHz, 16-bit, stereo capture and playback.
- `hid.0` — 64-byte vendor/consumer/telephony HID interface.
- `mass_storage.usb0` — exposes `/dev/udisk`.
- `acm.0` — CDC-ACM serial.
- `ffs.adb` — Android Debug Bridge FunctionFS.
- `ffs.mtp` — MTP support is defined but not fully enabled.

Available script modes include:

```text
start
restart
uac_cdc
adb_cdc
cdc_msc
uac_adb
uac_hid
uac_msc
uac_hid_adb
uac_hid_msc
uac_adb_msc
```

Normal startup enables UAC2 and HID. Debug mode uses the same visible combination.

### USB mode selection

`usb_mode_set` accepts internal mode values 0–9 and queues `thread_usb_mode_set`.
That thread builds a command of the form:

```sh
/etc/init.d/S89usbgadget <mode-name>
```

The recovered mapping is:

```text
1  uac_cdc
2  cdc_adb
3  uac_adb
4  cdc_msc
5  uac_hid
6  uac_msc
7  uac_hid_msc
8  cdc_msc
9  stop
```

The PC-Link OTA/U-disk transition path itself uses only the UAC/CDC/MSC modes.
However, entering factory mode through the PC-Link debug command does request
internal USB mode `2` (`cdc_adb`); leaving factory mode requests mode `0`.

### Audio function

The UAC2 function is configured with:

```text
capture:    2 channels, 48000 Hz, 2 bytes/sample
playback:   2 channels, 48000 Hz, 2 bytes/sample
volume:     -100 to 0, resolution 1
requests:   10
```

Although playback is configured, the Wave application presents the device primarily as a microphone/conference audio source.

## HID descriptor

The active descriptor is a composite of three application collections:

1. **Vendor-defined**, report ID `3`.
2. **Telephony**, report ID `2`.
3. **Consumer**, report ID `1`.

The vendor collection is:

```text
Usage Page: 0xFF00
Usage:      0x01
Collection: Application
Report ID:  3
Input:      63 bytes, 8-bit each
Output:     63 bytes, 8-bit each
End Collection
```

The full report size is 64 bytes: 1 report-ID byte plus 63 payload bytes. The firmware's internal vendor-package structure is 88 bytes; its payload begins at byte 17 and is followed by timing/retry bookkeeping.

## Vendor HID transport

### Active 64-byte wire report

Tracing `hid_transfer_message_handler` through `recv_hid_data` and
`change_carray_to_hid_vendor_package` establishes the active PC-Link format:

```text
Offset  Size  Meaning
0x00    1     Report ID, 0x03
0x01    1     Message type
0x02    8     Packet ID, little-endian uint64
0x0A    1     Payload length, maximum 52
0x0B    52    Payload
0x3F    1     Unused/padding
```

`send_mavlink_message_to_hid` splits the MAVLink byte stream into 52-byte
chunks, incrementing packet IDs for each chunk. Incoming normal reports append
to a continuous MAVLink parser; there is no fragment-final index in this format.
This format was verified with live device info/status and audio commands.

Recovered message types:

```text
00  MAVLink data
01  Resend request (packet ID identifies the missing report)
02  Resend response when the requested report is unavailable
03  Reset transport queues/parser
04  Reset response
05  Synchronize received packet ID
06  Synchronization response
A1..A5  Camera cascade handshake/control/reboot traffic
EC  Short heartbeat request
ED  New short heartbeat request
EF  Short heartbeat response
F0  Factory switching
```

The firmware maintains send/receive queues, a sent-report pool, lost-report
lists, duplicate suppression, and retry counters. A host must preserve packet
order, answer resend requests from its sent-report history, and request missing
incoming reports. Transport reset starts a new HID stream; it does not reboot
or factory-reset the device. A running official controller can compete with a
CLI for packet IDs and sessions.

### Short heartbeat reports

Short heartbeats use a separate layout, not the 11-byte data header:

```text
0       Report ID 3
1       Message type EC/ED/EF
2       State length, 58
3..60   Packed status state
61..63  Padding
```

`transfor_short_heartbeat_to_carray` emits an `EF` response containing the
58-byte state. The host echoes the received state using this response format.
Sending a zero-filled 64-byte report with prefix `03 ED` requests current state;
the Wave replies `03 EF 3A` plus 58 bytes. This was verified on live hardware.
Camera fields in that state are:

| State offset | Size | Meaning |
| --- | --- | --- |
| 2 | 1 | Camera connection state; 1 means connected |
| 32 | 1 | Setting mode: 0 off, 1 single tracking, 2 group tracking, 3 whiteboard, 4 desk view |
| 33 | 1 | Privacy enabled |
| 34 | 1 | Camera UUID color flag, despite DWARF naming it extended privacy |
| 35 | 2 | Little-endian zoom, 100–400 |
| 37 | 7 | Camera UUID, omitted from CLI status |

`reflash_heartbeat_status_info` at `0x1c568c` populates these fields from the
camera state/settings. In contrast, `slave_get_dev_status` initializes only the
low two camera state bits in its 318-byte reply. The connection bit, camera error
bits, mode, zoom, and privacy fields are left untouched. The CLI reports those
undefined fields as `null` when decoding this reply alone and replaces camera
status with a separate short heartbeat in the USB client.

### Internal vendor package

DWARF `_hid_vendor_package` and conversion functions identify:

```text
Offset  Size  Meaning
0x00    1     Report ID
0x04    4     Message type enum
0x08    8     Packet ID
0x10    1     Payload length
0x11    52    Payload
0x48    8     Last-send time
0x50    1     Send count
0x51    1     Used flag
0x52    1     Disabled flag
```

Remaining bytes are alignment/padding. This is in-memory bookkeeping, not a
second command payload.

### Separate fragment parser

`parse_hid_payload_to_mavlink_payload` also exists, but is not the active
`recv_hid_data` path traced above. Its layout is `03, length, final-index,
current-index, data[60]`. The first fragment has **byte 3** equal to 1. Earlier
notes reversed these index bytes and treated this parser as the active USB
transport. Do not use that layout for the CLI's normal PC-Link connection.

## MAVLink payload layer

After HID reassembly, the data is parsed as MAVLink 2-style messages. The firmware contains generated MAVLink encode/decode/pack functions for these Insta360 messages:

```text
heartbeat_control
heartbeat_notify
puc_cmd_detect
puc_cmd_ptz
puc_cmd_zoom
puc_cmd_track
puc_cmd_privacy
puc_cmd_doa_enable
puc_cmd_doa_info
puc_cmd_doa_info_v2
puc_cmd_version
puc_cmd_white_board
puc_cmd_white_board_result
puc_cmd_reset
puc_cmd_get_uvc_frame_num
puc_cmd_get_device_info
puc_cmd_deskview
puc_cmd_get_cfg_info
```

The Wave can forward these commands to a connected PUC camera over its UART/PUC transport. The camera-facing command wrappers include:

```text
puc_cmd_ptz_get
puc_cmd_set_ptz
puc_cmd_get_zoom
puc_cmd_set_zoom
puc_cmd_zoom_add
puc_cmd_zoom_sub
puc_cmd_get_track
puc_cmd_set_track
puc_cmd_get_privacy
puc_cmd_set_privacy
puc_cmd_set_ext_privacy
puc_cmd_get_doa_status
puc_cmd_set_doa
puc_cmd_set_doa_angle
puc_cmd_get_system_info
puc_cmd_get_devinfo
puc_cmd_get_uvc_frame_num
puc_cmd_set_reset
puc_cmd_set_deskview
puc_cmd_set_white_board
puc_cmd_disable_cur_setting_mode
```

The active PC-Link layer uses MAVLink 2 message `pc_cmd_pkg`, ID **1000**,
with a 128-byte field and CRC extra **248**. Heartbeat control is ID **0**,
32 bytes, CRC extra **69**; heartbeat notify is ID **1**, 32 bytes, CRC extra
**32**. Firmware packing uses system/component IDs 0/0. MAVLink 2 can truncate
trailing zero bytes; restore the fixed-size field before decoding PC-Link.

The CRC includes the header after the start byte, the transmitted payload, and
the message's CRC extra. See the [MAVLink serialization documentation](https://mavlink.io/en/guide/serialization.html).
Camera message field mappings still need implementation/validation.

## PC-Link command layer

The HID/MAVLink transport also carries the Wave's PC-Link protocol. The firmware registers handlers named `pc_link_slave_recv_pkt_handle_*`.

### Recovered PC-Link packet

DWARF `pc_link_pkt_t` defines a 16-byte little-endian header followed by up to
112 bytes of command payload inside the 128-byte MAVLink field:

```text
0..3    Session token (default 0x4748494A; authenticated sessions can differ)
4       Protocol version, 1
5       Flags: bits 0..1 packet type, 2..3 ACK, 4 encryption,
              5..6 packet end, 7 reserved
6..7    Command ID, uint16
8..11   Packet sequence, uint32
12..15  Command payload length, uint32
16..127 Payload, padded to 112 bytes
```

Packet type 0 is request; type 1 is response. End flag 2 starts a multipart
response, 0 continues it, and 1 finishes it (also used for single packets).
Live response sequences use the sentinel `0xffffffff`; order is enforced at
the outer HID packet layer. Requests increment their own PC-Link sequence.
Empty queries use a full 112-byte `ff` padding payload with length 112.
Multipart requests use end flags 2/0/1 too, with a new PC-Link sequence for each
packet (confirmed in `pc_link_create_cmd_pkt_headerinfo`). A single request uses
end flag 1. Integer fields in the command structures are packed little endian.

Implemented command IDs and payloads:

| Command | ID | Request | Reply |
|---|---:|---|---|
| Device info | 0 | Padding block | 933-byte packed info |
| Speaker volume | 38 | uint8, 0–100 | uint8 result, 0 success |
| Speaker mute | 40 | uint8, 0/1 | uint8 result, 0 success |
| Mic volume | 41 | uint8, 0–100 | uint8 result, 0 success |
| Mic mute | 43 | uint8, 0/1 | uint8 result, 0 success |
| Device status | 64 | Padding block | 318-byte packed status |

Audio status: byte 150 bits 6/7 are speaker/mic mute; bytes 151/152 are
speaker/mic volume. These fields were verified by changing each value, reading
status, and restoring the original value. Info includes binding/cloud token
fields; the CLI deliberately excludes those from its output.

### Additional CLI command coverage

The command IDs below come from `pc_link_cmd_e` and the 62-entry slave-handler
table. Structure sizes/offsets come from the ELF's DWARF types. See
`src/wave_command_protocol.rs` for exact reply offsets and `src/wave_commands.rs`
for request encoding. The CLI now exposes every registered handler except file
transfer (71), plus the separately recovered factory debug operation (128).

| Group | IDs | Packed requests / replies |
|---|---|---|
| Account | 1, 2, 16–22 | device token 512; verification 33; binding record 1216; authentication 80; unbind flag 1; binding challenge 17, auth reply 85, binding info 722 |
| Wi-Fi | 23–30, 145 | connect 129; disconnect 65; SSID 64; switches 1; network list 4290, status 133, base info 129 |
| Meetings | 31–37, 72 | operation/mark/mode 1; meeting ID 2; screenshot 260; desktop state 5; status 517, info 644, current metadata 801 |
| Audio | 38–43 | volume/mute/mode 1; result 1 |
| Settings | 45–56, 58–63, 144 | suggested language 64; language+country 128; switches/modes 1; locale 64; time 24; datetime preferences 9; base info 133 |
| OTA | 67–70 | empty mode transition; package operation 129; completion status 1; status reply 4 |
| Camera | 73–76 | zoom/reset/direction/mode 1; ACK 1 |
| Factory mode | 128 | `47 00` or `47 01`; reply 2 |

The settings catalogs contain fixed arrays of 64-byte strings: 20 languages,
260 countries, and 120 timezones. Language/country/timezone replies are 1281,
16642, and 7682 bytes; the combined catalog is 25606 bytes. Counts are checked
against array capacity before reading strings. Time uses six uint32 fields
(year, month, day, hour, minute, second). Datetime preferences use display mode
and automatic sync bytes, uint16 year, then five single-byte calendar fields.

Meeting operations in `slave_meeting_operate` are 0=new recording, 1=finish,
2=pause, 3=resume. These differ from the internal meeting-server command enum.
Marks are 0=common, 1=task, 2=image. Meeting mode is 0=record, 1=transcript only.
Password command 17 uses 0=request verification on the device UI and 1=poll;
the callback ignores its password string. Its result is 0=pending, 1=verified,
2=failed. It must not be interpreted as an ordinary zero-success ACK.

Camera zoom 0 subtracts and nonzero adds. Direction 0 sends `(0,0)`, 1 `(-8,0)`,
2 `(8,0)`, 3 `(0,8)`, 4 `(0,-8)` to `puc_cmd_set_ptz`. Camera mode 0 disables
the current mode, 1/2 invoke tracking submodes 0/1, 3 enables whiteboard, and
4 enables desk view. Firmware `PUC_SETTING_MODE_SIG_TRACK` and
`PUC_SETTING_MODE_MUTI_TRACK` identify modes 1/2 as single/group tracking.
Live tests verified zoom steps of 10 and mode readback for values 0–4.
Reset/direction callbacks store the boolean return of the PUC wrapper directly:
their ACK is **1=accepted, 0=failed**, unlike ordinary zero-success replies.
This only confirms command acceptance; USB PC-Link has no position getter.

**Desk-view exit quirk:** mode 0 while in mode 4 produces optimistic off readback,
but it reverts to 4 after several seconds. This reproduces even with one persistent
USB session. Mode 4→1→0 remains off after six seconds, so the client inserts mode 1
before mode 0 only when leaving desk view. This briefly enables single tracking.
`puc_server_thread` compares local and synchronized modes and adopts synchronized
state once both timestamps exceed 3000 ms (`0x1b2420`–`0x1b2550`), explaining why
immediate readback alone is insufficient. The live regression exercises this
delayed behavior and restores the previous mode afterward.

**Uninitialized reply fields:** `slave_get_bind_info` writes the binding fields
but never writes byte 0 (`result`), and its caller does not initialize the buffer.
The CLI validates the binding flag and ignores this undefined result byte.
`slave_puc_set_zoom` and `slave_puc_set_mode` similarly do not write their ACK bytes.
The mode callback returns a result to its caller, but the caller discards it and
sends the untouched ACK buffer. `slave_ota_finish_sure`
leaves its ACK undefined when the supplied status is zero. The latter controls
therefore report receipt with `verified: false`; their ACK cannot prove success.
Other decoded result fields require zero unless a specific enum is documented.

The PUC MAVLink command vocabulary above belongs to the camera UART channel.
`pc_mavlink_recv_pkt_cb` accepts only message IDs 0 and 1000 for incoming USB
control; it does not forward IDs 2000–2016 to the camera. Absolute PTZ/zoom,
privacy writes, and camera identity queries therefore have no recovered ordinary
USB PC-Link route.

**Large-reply loss:** `append_hid_vendor_package_to_send_list_tail` at `0x1b9e2c`
limits the send list to 400 entries. When full, it removes the oldest queued entry
before appending another. These unsent reports never reach the already-sent pool
(1200 entries in `save_hid_vendor_package_to_already_send_pool`). Requests for
them return HID type 2, making reassembly impossible. Encrypted country and
combined settings catalogs require roughly 447 and 687 reports respectively.
Both failed this way in live validation. A 7682-byte timezone reply also suffered
unrecoverable loss; that smaller case needs further diagnosis. The CLI supports
32 KiB assembly, handles loss within the sent pool, and fails explicitly rather
than returning partial catalogs. Enlarging the host HID queue did not fix it;
no patched HIDAPI dependency is retained.

The enum includes IDs 44, 57, 65, and 66 without registered handlers. The CLI
does not offer them. Use the full status or system base-info query for current
speaker volume and timezone. OOB commands 112/113 belong to transfer recovery,
not ordinary user commands.

### Session encryption and binding lookup

`pc_link_update_bind_status` copies the binding key and calls
`pc_link_set_encrykey_userid` with that **key** (despite the helper name). The
helper copies its first 16 bytes as the AES key. `aesEncrypt`/`aesDecrypt` apply
AES-128 ECB to the complete 112-byte payload, without PKCS padding; the header
stays plaintext. Requests pad unused payload bytes with `ff` before encryption.

Commands 0–16, file transfer 71, and debug 128 are plaintext exceptions even
when the header's encryption bit is set. This explains the encrypted flag on
plaintext device-info replies. Do not use `bindUserid` as the AES key.

The macOS Wave Controller stores a login token in its `startup.ini` and obtains
an existing binding key with a read-only authenticated GET:

```text
<regional summary-service>/deviceBinding/getOneDeviceBindInfo?deviceId=<device SN>
X-User-Token: <controller login token>
response: {"code": 0, "data": {"bindKey": "...", ...}}
```

This was recovered from `PspDevice::GetBindingKeyFormCloud` in the installed
Wave Controller and verified against the signed-in account. The Rust CLI performs
this lookup only through `wave binding-key`, using `--controller-login` or an
existing `WAVE_LOGIN_TOKEN`. That explicit command writes the key to stdout for
shell capture. USB controls consume `WAVE_BIND_KEY` without cloud access. The
CLI never saves credentials or invokes the binding/unbinding endpoints.


### Device and binding

```text
get_dev_info
get_dev_status
get_bind_start
get_bind_info
bind_dev
auth_dev
unauth_dev
unbind_dev
auth_unbind_dev
unbind_offline
verify_dev_passwd
debug
```

Binding credentials include user ID, bind key, bind area, and cloud-service token. Device status exposes firmware/MCU versions, model, USB ID, battery, Wi-Fi state, audio model type, and debug/factory mode.

### Debug commands

`pc_link_slave_recv_pkt_handle_debug` forwards a two-byte command payload to
`slave_debug`, which is registered through the PC-Link callback table.

The recovered commands are:

```text
47 00  leave factory mode
47 01  enter factory mode
```

`slave_debug` calls `gui_set_factory_mode(0)` or `gui_set_factory_mode(1)`.
The mode is persisted in the `UNIFYKEYS_SYS_DEVMODE` key. Factory mode enables
the GUI/debug switch, requests the `cdc_adb` USB composition, suppresses many
user-facing toasts, and changes several normal-user behaviors. It does not
start a shell, Telnet, HTTP, or an arbitrary command handler.

### Meetings

```text
meeting_op
get_meeeting_status
get_meeting_info
get_cur_meeting_all_info
meeting_mark
meeting_screenshot
meeting_mode_set
meeting_pc_status
```

### Audio

```text
audio_speaker_mode
audio_speaker_volume
audio_speaker_mute
audio_mic_mode
audio_mic_volume
audio_mic_mute
```

High-level helpers also exist for:

```text
volume_up
volume_down
mute
play_pause
music_next
music_prev
reject_ring
pick_up_meeting
hang_up_meeting
mic_mute
mic_unmute
```

### Camera/PUC control

```text
puc_set_direction
puc_set_mode
puc_set_pos_reset
puc_set_zoom
```

These map onto the PUC/MAVLink command set above.

### System

```text
system_base_info
sys_language
set_sys_country
get_sys_language
get_sys_country
sys_get_language_list
sys_get_language_country_list
sys_get_all_setting_list
sys_get_default_setting
sys_get_country_timezone_list
sys_get_timezone_city
sys_timezone
sys_time
sys_set_date_time_setting
sys_set_time_auto_sync
sys_set_time_show_mode
sys_set_enable_udisk
sys_reboot
sys_recovery
```

### Wi-Fi

```text
wifi_base_info
wifi_scan
wifi_list
wifi_connect
wifi_disconnect
wifi_status
wifi_forget_passwd
wifi_switch
wifi_auto_connect_switch
```

Connect commands carry SSID, password, and encryption mode.

### File transfer and OTA

```text
file_trans
ota_op
ota_status
ota_finish_sure
ota_swt_udisk_cdc
```

The file-transfer wire header is three uint32 fields: length, CRC, and slot index,
followed by file bytes. `pc_link_recv_msg_pkt` creates filenames locally for slots
0–4 rather than receiving an arbitrary filename in this header. Its staging,
CRC, and out-of-band recovery paths still need a complete host implementation;
the CLI does not expose a partial uploader. OTA can switch the Wave between
UAC/HID and mass-storage/CDC modes, run filesystem checks on `/dev/udisk`, and
report package status. Command 68 carries an operation byte and a 128-byte
package filename; the callback ignores the operation byte and starts the staged
package under `/mnt/udisk/misc/data/controlsystemfile/`. The CLI supplies zero
for that unused byte and restricts input to a basename.

## Wave/Link cascade handshake

The firmware supports a Wave/Link 2 cascade mode over HID. Camera attachment does
not always activate this mode: live testing with a paired Link 2 plugged directly
into the Wave successfully exercised ordinary PC-Link camera commands.

Recovered handshake functions:

```text
vb_parse_resp_handshark_step1_command
vb_parse_resp_handshark_step2_command
send_vb_cascade_handshark_step1_response
send_vb_cascade_handshark_step2_response
send_vb_cascade_reboot_sync_response
```

Additional cascade operations include:

```text
send_psp_volume_sync_request
send_psp_sleep_wake_notify
hid_server_save_volume_before_cascade
user_request_vb_psp_cascade_set_volume
user_request_vb_psp_cascade_set_speaker_mute
user_request_vb_psp_cascade_set_mic_mute
```

During cascade mode, the firmware explicitly ignores ordinary PC-Link and MAVLink control commands. This is relevant to the intermittent paired-camera crash: a valid cascade HID stream and correctly sequenced heartbeat/handshake are prerequisites for stable camera control.

## Heartbeat

The transport has both normal and short heartbeat paths:

```text
get_short_heartbeat_request_to_hid
get_short_heartbeat_response_to_hid
is_short_heartbeat_match
transfor_carray_to_short_heartbeat
thread_of_pclink_client_hearbeat_check_task
```

Heartbeat status tracks volume, mute, sleep, USB insertion, and cascade state. If heartbeat state diverges or the cascade handshake is interrupted by USB removal, the firmware resets the handshake/cascade flags.

## macOS access notes

1. Enumerate the device with USB VID `0x18F0`, PID `0x4E40`.
2. Claim the HID interface and use report ID `3`.
3. Send and receive fixed 64-byte reports, including report ID 3 exactly once.
4. Use the active 11-byte HID header and 52-byte MAVLink chunks.
5. Reassemble the MAVLink stream in packet-ID order and handle duplicate/lost reports.
6. Decode PC-Link and use the binding key for encrypted status/audio commands.

On macOS, the `IOHIDDevice` API requires a report buffer large enough for the report ID plus 63 payload bytes.

## Open questions

- Camera MAVLink field semantics and live control validation.
- Remaining numeric audio/status/platform enum meanings and live validation of new writes.
- Reliable large catalog delivery, including the smaller timezone loss case.
- Complete bulk transfer/CRC/resumption behavior.
- Cloud/device coordination for the complete authentication/binding lifecycle.
- Cascade handshake command IDs and payload fields.
- Exact retry intervals and heartbeat timing.

## Native macOS controller evidence

The SwiftUI app in `macos/` bundles the existing USB CLI. Static inspection of
the official macOS controller's embedded QML and ARM64 symbols established:

- `micMode == 1` is noise reduction. The original-sound list uses `index + 2`,
  in this order: omni, cardioid, supercardioid, figure-eight, stereo (2–6).
- Live command-64 readback places the three-bit recording state across byte 155
  bit 7 and byte 156 bits 0–1: `(data[155] >> 7) | ((data[156] & 3) << 1)`.
  Sanitized byte pairs were idle `[4, 24]`, recording `[132, 24]`, paused
  `[4, 25]`, and stopping `[4, 26]`; duration at bytes 157–160 advances while
  recording. `PSP::PspDevice::GetMeetingStatus` masks a repacked in-memory field,
  which cannot be used directly as the wire layout. The decoder regression test
  covers these live samples and isolates the preceding input flags.
- Embedded recording controls label states 0/3 as ready, 1 as recording, 2 as
  paused, and 4 as stopping. Other states are treated as unknown/non-actionable.
- `CloudUrls::updateUrls` appends the desktop `/upload/pc/` paths to the supplied
  base. `UploadScreenShot` supplies the configured endpoint plus `/deviceBinding`.
  `meetingInit` posts to `uploadInit`; `meetingFileUpload` requests
  `getUploadSignUrl`, performs the signed object PUT, and reports success through
  `reportFileUploadStatus` before USB screenshot registration. This desktop flow
  is implemented in the app but is not yet validated end to end on a live meeting.

The app imports existing keys through macOS’s built-in HTTPS client. During
validation, the Rust HTTP stack and URLSession repeatedly timed out while curl
returned a valid key for the same account and endpoint. Authentication is passed
through private stdin and redirects are refused. No secrets were retained in the
repository.
