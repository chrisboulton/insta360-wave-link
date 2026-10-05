# Wave remote recording control research

Investigated 2026-10-04. Scope: public Insta360 documentation and the already
extracted firmware under `extracted/Wave/rootfs`. No device configuration changes,
recording commands, network scanning, or private account/configuration reads were
performed for this investigation.

## Conclusion

**Direct LAN recording control on the consumer Wave is not verified.** There is
substantial static evidence of a remote recording integration in the firmware,
but the identified control router binds to loopback, not the LAN. Its external
counterpart, activation requirements, authentication, and availability on the
consumer device remain unresolved. Classify this as a **promising lead**, not a
working device transport and not proof that network control is impossible.

The existing USB PC-Link path is verified and can support a Mac-to-Mac host/client
bridge without depending on that unresolved firmware integration. See
[the USB protocol notes](wave-usb.md) for the implemented meeting operations and
status readback.

## Official support: consumer and Business are different claims

- The [consumer Controller instructions](https://onlinemanual.insta360.com/wave/en-us/camera/advanced-use/install-wave-controller)
  specify USB-C for the first connection, then desktop volume/recording control.
  They describe Wi-Fi and cloud sync as uploading completed recordings to
  InSight. This page does **not** describe a consumer LAN control interface.
  Absence from this page is not proof of absence in the device.
- The [Wave For Business API page](https://onlinemanual.insta360.com/wave-for-business/en-us/camera/advanced-use/api)
  explicitly advertises remote power, recording start/pause/end, pickup-pattern,
  audio-quality, and volume controls, including scheduled meeting automation.
  It directs integrators to Insta360 for the complete interface documentation.
  It does not publish endpoints, credentials, transport, or a claim that the API
  is available on consumer Wave hardware/accounts. It also does not establish
  that the API is direct LAN rather than a managed/cloud integration.
- The [Business specifications](https://onlinemanual.insta360.com/wave-for-business/en-us/camera/specifications)
  list API access for device control/status and meeting/file management.
- Insta360's [Wave SDK documentation source](https://github.com/Insta360Develop/Insta360-Developer_Docs/blob/main/docs/en/wave/index.md)
  says the SDK/API is not public yet. Other Insta360 product APIs cannot be
  assumed to apply to Wave.

## Firmware evidence

### A real remote-recording integration exists in compiled code

Source: `extracted/Wave/rootfs/usr/lib/libins_entry.so`, an unstripped AArch64 ELF
with symbols and DWARF information. The compilation-unit name is
`src/server/mqtt_client/mqtt_client_server.c`.

Concrete symbols:

| Symbol | Address | Evidence |
| --- | --- | --- |
| `mqttcli_server_init` | `0x1cd0c8` | Calls `mqa_zmq_server_init`, then registers status callbacks. |
| `mqttcli_server_request` | `0x1cd274` | Handles local integration requests and upload coordination. |
| `mqttcli_server_status_collection_cb` | `0x1cee3c` | Reports recording state and local/remote operation results. |
| `handle_request_message` | `0x1d2bdc` | Contains remote record/control message handling. |
| `enhanced_server_create` | `0x1d8cac` | Creates the CZMQ ROUTER socket. |
| `mqa_zmq_server_init` | `0x1d9014` | Creates the router and starts its request-processing thread. |

Recovered command/log vocabulary includes `ZMQ_CMD_RECORD`, `ZMQ_CMD_RECORD_RESULT`,
`ZMQ_CMD_POWER`, `ZMQ_CMD_TRANSLANG`, activation/binding messages, and recording
states `begin`, `pause`, `resume`, `stop`, `remind`, and `cancel`. The code includes
record-ID checks for pause/resume/stop, a thread that starts recording after an
existing recording finishes, and local-versus-remote recording status reports.
Database/status names include `MQTTCLI_SERVER_ADDRESS`, `MQTTCLI_DEV_ACTIVE_STATUS`,
`GUI_REQUEST_MQTTCLI_SERVER_CONNECT`, and `MQTTCLI_RECORD_STATUS`.

These establish compiled support for some remote integration. They do not
establish its runtime activation or a usable public request schema.

### The identified remote-control router is internal TCP IPC

`enhanced_server_create` loads the constant at `0x256a00` and calls
`zsock_new_router_checked` at `0x1d8da4`:

```text
tcp://127.0.0.1:6611
```

That is a loopback endpoint. It is not a direct LAN listener. The firmware's
MQTT-client naming and server-address/activation fields suggest an integration
component communicates through this local router, but the external broker/server
transport and its implementation were not established in this bounded pass.
In particular, this evidence does not identify a consumer-accessible MQTT topic,
broker, authentication method, or HTTP endpoint.

### The wildcard HID sockets do not themselves establish remote control

The same library also has network sockets associated with the USB HID manager:

| Component | Socket role | Endpoint | Meaning from static evidence |
| --- | --- | --- | --- |
| `thread_of_send_to_hid_server` in `libins_entry.so` | CZMQ PUB | `tcp://*:5533` | Publishes data toward the HID manager. Creation at `0x1c7acc`. |
| `thread_of_handle_hid_recv_data` in `libins_entry.so` | CZMQ SUB | `tcp://127.0.0.1:5544` | Receives data from the local HID manager. Creation at `0x1c7f20`. |
| `usr/bin/hid_manager_server` | Matching endpoint strings | `tcp://127.0.0.1:5533`, `tcp://*:5544` | Counterpart of that HID interprocess link. |

The wildcard addresses are worth remembering for future live inspection, but
the library's wildcard socket is a **publisher**, and its command-receiving
subscriber connects to loopback. Merely finding TCP ports does not provide a
way for a LAN client to inject meeting commands. Reachability, startup paths,
runtime binding, framing, and security were not tested.

### A web recording page is an Amlogic demo, not recovered Wave control

Files `var/www/record.html` and
`var/www/cgi-bin/record/record_ctl.sh` form a generic Amlogic recording demo.
The shell handler starts `arecord -Dhw:0,3`, stops it with `killall`, and uses
`aplay` for playback. It writes relative WAV files and does not call the Wave
meeting manager, PC-Link handler, cloud meeting initializer, or MQTT integration.

No HTTP daemon launch was found in the inspected `etc/init.d` scripts or
`etc/inittab`. This packaged demo does not establish that an HTTP server runs on
the consumer device, and it should not be used as a Wave recording API.

The Telnet respawn line in `etc/inittab` is commented out. `etc/inetd.conf` has a
Telnet service entry, but no inspected init script launches inetd. Those files
likewise do not prove an active service. A `lvgl_touch_bridge` socket implementation
is also present; it is a UI/debug lead, not established LAN meeting control.

### Outbound cloud HTTP is separate

`usr/lib/libcloud_proto_https.so` implements the outbound device/cloud requests
already documented in [the HTTP notes](wave-http.md): meeting initialization,
transcription notices, upload preparation/status, device binding, and OTA.
Those client endpoints do not constitute a device-local listening API. This
investigation found no justified consumer LAN HTTP/WebSocket recording endpoint
to implement, and no Wave Bonjour advertisement in the inspected init/config
and targeted application/library strings. This was a targeted pass, not a proof
that no such code exists anywhere in the image.

## Remaining verification that would change the conclusion

1. Identify which component talks to `127.0.0.1:6611`, what activates it, and
   whether that integration is gated to Business mode, firmware, or enrollment.
2. Obtain the official Business API specification and determine whether its
   transport is direct LAN, outbound MQTT/cloud, or a separate controller.
3. With the device's actual address explicitly known, inspect only the candidate
   sockets and runtime process/listener state read-only; do not infer an API from
   a successful TCP connection alone.
4. Recover authentication and message framing before attempting any write. A
   remote `begin` path can finish an existing recording before starting another,
   so speculative write probes would not be harmless.

Until those are resolved, the Mac USB host bridge is the concrete route for the
requested remote start/stop behavior. It would control the recorder on the Wave;
it would not transport the meeting computer's microphone/audio to the Wave.
