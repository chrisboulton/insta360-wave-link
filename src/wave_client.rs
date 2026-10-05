//! Synchronous HID client. All device control stays on USB and never requires a login token.

use crate::wave_protocol::{
    WAVE_PC_COMMAND_ID, WAVE_PRODUCT_ID, WAVE_SESSION_TOKEN, WAVE_VENDOR_ID, WaveBindingKey,
    WaveHidReport, WaveMavlinkDecoder, WavePcPacket, decode_wave_camera_heartbeat,
    decode_wave_info, decode_wave_status, encode_wave_command_packets, encode_wave_mavlink,
    wave_command_is_encrypted,
};
use anyhow::{Result, bail, ensure};
use hidapi::{DeviceInfo, HidApi, HidDevice};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

// The firmware's already-sent pool contains 1200 reports (105608 bytes including metadata).
// Large catalogs span hundreds of reports, so a 128-report window cannot recover them.
const WAVE_HID_RECOVERY_REPORTS: usize = 1200;

fn wave_device_matches(device: &DeviceInfo, serial: Option<&str>) -> bool {
    device.vendor_id() == WAVE_VENDOR_ID
        && device.product_id() == WAVE_PRODUCT_ID
        && serial.is_none_or(|serial| device.serial_number() == Some(serial))
}

/// Enumerate Wave HID interfaces without opening a device or reading credentials.
pub fn list_wave_devices(serial: Option<&str>) -> Result<Vec<Value>> {
    let api = HidApi::new()?;
    Ok(api.device_list().filter(|device| wave_device_matches(device, serial)).map(|device| {
        json!({"path": device.path().to_string_lossy(), "vendor_id": device.vendor_id(),
            "product_id": device.product_id(), "serial_number": device.serial_number(),
            "manufacturer_string": device.manufacturer_string(), "product_string": device.product_string(),
            "release_number": device.release_number(), "interface_number": device.interface_number(),
            "usage_page": device.usage_page(), "usage": device.usage()})
    }).collect())
}

#[derive(Default)]
struct WaveHidStream {
    packet_id: u64,
    mavlink_sequence: u8,
    last_received_id: Option<u64>,
    received_pending: BTreeMap<u64, Vec<u8>>,
    requested_missing_id: Option<u64>,
    sent_reports: BTreeMap<u64, WaveHidReport>,
    decoder: WaveMavlinkDecoder,
    camera_heartbeat: Option<Value>,
}

struct WaveReceivedMessages {
    outgoing: Vec<[u8; 64]>,
    messages: Vec<(u32, Vec<u8>)>,
}

impl WaveHidStream {
    fn send_pc_packet(&mut self, packet: &[u8]) -> Result<Vec<[u8; 64]>> {
        let frame = encode_wave_mavlink(WAVE_PC_COMMAND_ID, packet, self.mavlink_sequence)?;
        self.mavlink_sequence = self.mavlink_sequence.wrapping_add(1);
        let mut outgoing = Vec::new();
        for chunk in frame.chunks(52) {
            self.packet_id = self
                .packet_id
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Wave HID packet ID overflow"))?;
            let report = WaveHidReport {
                kind: 0,
                packet_id: self.packet_id,
                payload: chunk.to_vec(),
            };
            outgoing.push(report.encode_report()?);
            self.sent_reports.insert(self.packet_id, report);
            if self.sent_reports.len() > WAVE_HID_RECOVERY_REPORTS {
                self.sent_reports.pop_first();
            }
        }
        Ok(outgoing)
    }

    fn receive_report(&mut self, wire: &[u8]) -> Result<WaveReceivedMessages> {
        let mut result = WaveReceivedMessages {
            outgoing: Vec::new(),
            messages: Vec::new(),
        };
        if wire.is_empty() || wire[0] != 3 {
            return Ok(result);
        }
        ensure!(wire.len() == 64, "Wave HID report has invalid size");
        if [0xec, 0xed, 0xef].contains(&wire[1]) {
            ensure!(
                wire[2] == 58,
                "Wave short heartbeat has unsupported state length"
            );
            self.camera_heartbeat = Some(decode_wave_camera_heartbeat(&wire[3..61])?);
            if wire[1] == 0xef {
                return Ok(result);
            }
            let mut reply = [0; 64];
            reply[..3].copy_from_slice(&[3, 0xef, 58]);
            reply[3..61].copy_from_slice(&wire[3..61]);
            result.outgoing.push(reply);
            return Ok(result);
        }
        let report = WaveHidReport::decode_report(wire)?;
        let mut reply = None;
        match report.kind {
            1 => {
                reply = Some(
                    self.sent_reports
                        .get(&report.packet_id)
                        .cloned()
                        .unwrap_or_else(|| WaveHidReport::control(2, report.packet_id)),
                )
            }
            2 if self.requested_missing_id == Some(report.packet_id) => bail!(
                "Wave cannot retransmit a missing HID report; the reply was discarded. Large catalogs can overflow the firmware's 400-report send queue."
            ),
            5 => {
                if let Some(last) = self.last_received_id {
                    reply = Some(if report.packet_id <= last {
                        WaveHidReport {
                            kind: 6,
                            packet_id: report.packet_id,
                            payload: vec![0],
                        }
                    } else {
                        WaveHidReport::control(1, last + 1)
                    });
                }
            }
            3 => {
                self.decoder = WaveMavlinkDecoder::default();
                self.last_received_id = None;
                self.received_pending.clear();
                self.requested_missing_id = None;
                reply = Some(WaveHidReport::control(4, 0));
            }
            0xa1..=0xa5 => bail!(
                "Wave is in camera cascade mode; disconnect the paired Link camera for PC-Link control"
            ),
            0 => {
                ensure!(
                    report.packet_id > 0,
                    "Wave HID data packet has invalid zero packet ID"
                );
                let mut last = self.last_received_id.unwrap_or(report.packet_id - 1);
                if report.packet_id <= last {
                    return Ok(result);
                }
                ensure!(
                    report.packet_id - last <= WAVE_HID_RECOVERY_REPORTS as u64,
                    "Wave HID stream jumped from {last} to {}; reconnect the Wave and retry",
                    report.packet_id
                );
                self.received_pending
                    .insert(report.packet_id, report.payload);
                while let Some(payload) = last
                    .checked_add(1)
                    .and_then(|id| self.received_pending.remove(&id))
                {
                    last += 1;
                    result.messages.extend(self.decoder.feed_payload(&payload)?);
                }
                self.last_received_id = Some(last);
                if self.received_pending.is_empty() {
                    self.requested_missing_id = None;
                } else if self.requested_missing_id != Some(last + 1) {
                    self.requested_missing_id = Some(last + 1);
                    reply = Some(WaveHidReport::control(1, last + 1));
                }
            }
            _ => {}
        }
        if let Some(reply) = reply {
            result.outgoing.push(reply.encode_report()?);
        }
        Ok(result)
    }
}

#[derive(Default)]
struct WaveReplyAssembler {
    payload: Vec<u8>,
    previous_sequence: Option<u32>,
    started: bool,
}

impl WaveReplyAssembler {
    fn accept_reply(&mut self, reply: &WavePcPacket) -> Result<bool> {
        match (reply.flags >> 5) & 3 {
            2 => {
                ensure!(
                    !self.started,
                    "Wave PC-Link response restarted unexpectedly"
                );
                self.started = true;
            }
            0 => ensure!(
                self.started,
                "Wave PC-Link response is missing its first packet"
            ),
            1 => {}
            _ => bail!("Wave PC-Link response has invalid end flag"),
        }
        // The firmware uses the same 0xffffffff sentinel for every response part.
        ensure!(
            self.previous_sequence
                .is_none_or(|sequence| sequence == reply.sequence),
            "Wave PC-Link response packet sequence changed unexpectedly"
        );
        self.previous_sequence = Some(reply.sequence);
        ensure!(
            self.payload.len() + reply.payload.len() <= 32_768,
            "Wave PC-Link response exceeds supported size"
        );
        self.payload.extend_from_slice(&reply.payload);
        Ok((reply.flags >> 5) & 3 == 1)
    }
}

/// Own one Wave vendor HID handle; drop closes it. Commands are synchronous and never retried.
pub struct WaveUsbClient {
    device: HidDevice,
    timeout: Duration,
    encryption_key: Option<[u8; 16]>,
    stream: WaveHidStream,
    command_sequence: u32,
    session_token: u32,
    info_loaded: bool,
    encryption_required: bool,
}

impl WaveUsbClient {
    /// Select the vendor interface and reset only transport queues, not device settings.
    pub fn open_device(
        serial: Option<&str>,
        timeout: Duration,
        binding_key: Option<&WaveBindingKey>,
    ) -> Result<Self> {
        ensure!(
            !timeout.is_zero() && timeout <= Duration::from_secs(60),
            "Wave USB timeout must be greater than 0 and at most 60 seconds"
        );
        let api = HidApi::new()?;
        let matching: Vec<_> = api
            .device_list()
            .filter(|device| wave_device_matches(device, serial))
            .collect();
        let mut candidates: BTreeMap<_, _> = matching
            .iter()
            .copied()
            .filter(|device| device.usage_page() == 0xff00 && device.usage() == 1)
            .map(|device| (device.path(), device))
            .collect();
        // Some Linux backends do not expose collection usage metadata.
        if candidates.is_empty() {
            candidates = matching
                .iter()
                .copied()
                .filter(|device| device.usage_page() == 0)
                .map(|device| (device.path(), device))
                .collect();
        }
        ensure!(
            !candidates.is_empty(),
            "Wave vendor HID interface not found; plug it into USB with a data-capable cable"
        );
        ensure!(
            candidates.len() == 1,
            "Wave selection is ambiguous; choose a device with --serial"
        );
        let device = api.open_path(candidates.first_key_value().unwrap().0)?;
        let client = Self {
            device,
            timeout,
            encryption_key: binding_key.map(WaveBindingKey::encryption_key),
            stream: WaveHidStream::default(),
            command_sequence: 0,
            session_token: WAVE_SESSION_TOKEN,
            info_loaded: false,
            encryption_required: false,
        };
        client.write_wire(&WaveHidReport::control(3, 0).encode_report()?)?;
        // Wait for the reset marker and discard stale reports from a previous invocation.
        // Sleeping here leaves reports queued and lets an old reply seed the new stream.
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let mut wire = [0; 64];
            let size = client.device.read_timeout(&mut wire, 100)?;
            if size == 64 && wire[0] == 3 && wire[1] == 4 {
                return Ok(client);
            }
        }
        bail!("Wave HID transport reset timed out; close Wave/Link Controller or reconnect USB")
    }

    fn write_wire(&self, wire: &[u8; 64]) -> Result<()> {
        ensure!(
            self.device.write(wire)? == wire.len(),
            "Wave HID write did not send the complete report"
        );
        Ok(())
    }

    /// Exchange a recovered PC-Link command. Replies are raw packed structures.
    /// State-changing commands are sent once; a timeout can follow a successful write.
    /// Exiting desk view first selects tracking, working around the firmware's broken direct exit.
    pub fn request_wave_command(&mut self, command: u16, payload: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            payload.len() <= 32_768,
            "Wave command exceeds supported request size"
        );
        if command != 0 && !self.info_loaded {
            self.read_wave_info()?;
        }
        ensure!(
            !wave_command_is_encrypted(command)
                || !self.encryption_required
                || self.encryption_key.is_some(),
            "Wave is account-bound; retrieve its key with 'wave binding-key --controller-login' and set WAVE_BIND_KEY for encrypted USB commands"
        );
        if (73..=76).contains(&command) {
            let camera = self.read_wave_camera_status()?;
            ensure!(
                camera["connected"] == true,
                "Wave camera is not connected; plug a compatible paired camera directly into the Wave"
            );
            // Direct desk-view exit reports off briefly, then camera synchronization restores 4.
            // Switching to tracking first makes the subsequent off request persist (live tested).
            if command == 76 && payload == [0] && camera["mode"] == 4 {
                crate::wave_command_protocol::decode_wave_command_reply(
                    76,
                    &self.request_wave_command(76, &[1])?,
                )?;
            }
        }
        let key = if self.encryption_required {
            self.encryption_key.as_ref()
        } else {
            None
        };
        let session = self.session_token;
        for packet in
            encode_wave_command_packets(command, payload, &mut self.command_sequence, session, key)?
        {
            for wire in self.stream.send_pc_packet(&packet)? {
                self.write_wire(&wire)?;
            }
        }
        let deadline = Instant::now() + self.timeout;
        let mut assembler = WaveReplyAssembler::default();
        while Instant::now() < deadline {
            let timeout_ms = deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .clamp(1, 100) as i32;
            let mut wire = [0; 64];
            let size = self.device.read_timeout(&mut wire, timeout_ms)?;
            let mut reports = vec![(wire, size)];
            // Drain the small native HID queue before any potentially blocking USB writes.
            for _ in 0..WAVE_HID_RECOVERY_REPORTS {
                let mut wire = [0; 64];
                let size = self.device.read_timeout(&mut wire, 0)?;
                if size == 0 {
                    break;
                }
                reports.push((wire, size));
            }
            let mut messages = Vec::new();
            let mut outgoing = Vec::new();
            for (wire, size) in reports {
                let incoming = self
                    .stream
                    .receive_report(&wire[..size])
                    .map_err(|error| anyhow::anyhow!("Wave command 0x{command:02x}: {error}"))?;
                messages.extend(incoming.messages);
                outgoing.extend(incoming.outgoing);
            }
            for wire in outgoing {
                self.write_wire(&wire)?;
            }
            for (message_id, message_payload) in messages {
                if message_id != WAVE_PC_COMMAND_ID {
                    continue;
                }
                // Filter unrelated encrypted notifications before attempting decryption.
                if message_payload.len() < 8
                    || u16::from_le_bytes([message_payload[6], message_payload[7]]) != command
                    || message_payload[5] & 3 != 1
                {
                    continue;
                }
                let reply =
                    WavePcPacket::decode_packet(&message_payload, self.encryption_key.as_ref())?;
                ensure!(
                    [0, 19].contains(&command) || reply.session == session,
                    "Wave PC-Link session changed; reconnect the Wave and retry"
                );
                if assembler.accept_reply(&reply)? {
                    if command == 0 {
                        ensure!(
                            assembler.payload.len() == 933,
                            "Wave device info has invalid response size"
                        );
                        self.info_loaded = true;
                        self.encryption_required = reply.flags & 0x10 != 0;
                        self.session_token = reply.session;
                    }
                    if command == 19 && assembler.payload.len() == 85 && assembler.payload[0] == 0 {
                        self.session_token =
                            u32::from_le_bytes(assembler.payload[1..5].try_into().unwrap());
                    }
                    return Ok(assembler.payload);
                }
            }
        }
        bail!(
            "Wave command 0x{command:02x} timed out; close Wave/Link Controller or reconnect USB. A write may have taken effect; read status before retrying."
        )
    }

    /// Read device identity without a key; omit credentials from the decoded output.
    pub fn read_wave_info(&mut self) -> Result<Value> {
        decode_wave_info(&self.request_wave_command(0, &[])?)
    }

    /// Read encrypted status on bound devices using only the supplied binding key.
    pub fn read_wave_status(&mut self) -> Result<Value> {
        let mut status = decode_wave_status(&self.request_wave_command(64, &[])?)?;
        let camera = self.read_wave_camera_status()?;
        status["camera_connected"] = camera["connected"].clone();
        status["camera"] = camera;
        Ok(status)
    }

    /// Request fresh camera status over the unencrypted short heartbeat transport.
    /// The firmware's PC-Link status reply leaves most camera fields uninitialized.
    pub fn read_wave_camera_status(&mut self) -> Result<Value> {
        // Drain queued notifications so an earlier heartbeat cannot satisfy this request.
        for _ in 0..WAVE_HID_RECOVERY_REPORTS {
            let mut wire = [0; 64];
            let size = self.device.read_timeout(&mut wire, 0)?;
            if size == 0 {
                break;
            }
            let incoming = self.stream.receive_report(&wire[..size])?;
            for wire in incoming.outgoing {
                self.write_wire(&wire)?;
            }
        }
        // ED with no state is a status request; never send invented state to the device.
        let mut request = [0; 64];
        request[..2].copy_from_slice(&[3, 0xed]);
        self.stream.camera_heartbeat = None;
        self.write_wire(&request)?;
        let deadline = Instant::now() + self.timeout;
        while Instant::now() < deadline {
            let mut wire = [0; 64];
            let timeout_ms = deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .clamp(1, 100) as i32;
            let size = self.device.read_timeout(&mut wire, timeout_ms)?;
            let incoming = self.stream.receive_report(&wire[..size])?;
            for wire in incoming.outgoing {
                self.write_wire(&wire)?;
            }
            if let Some(camera) = self.stream.camera_heartbeat.take() {
                return Ok(camera);
            }
        }
        bail!("Wave camera heartbeat timed out; close Wave/Link Controller or reconnect USB")
    }

    /// Set speaker volume/mute (38/40) or mic volume/mute (41/43), requiring a success reply.
    pub fn set_audio_control(&mut self, command: u16, value: u8) -> Result<()> {
        ensure!(
            match command {
                38 | 41 => value <= 100,
                40 | 43 => value <= 1,
                _ => false,
            },
            "Wave audio command or value is invalid"
        );
        let response = self.request_wave_command(command, &[value])?;
        ensure!(
            response == [0],
            "Wave audio command was rejected (invalid or unsuccessful acknowledgement)"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a paired camera and WAVE_BIND_KEY; temporarily changes camera modes"]
    fn live_camera_can_exit_desk_view_after_settings_synchronize() -> Result<()> {
        let key = std::env::var("WAVE_BIND_KEY")
            .ok()
            .map(WaveBindingKey::parse_key)
            .transpose()?;
        let mut client = WaveUsbClient::open_device(None, Duration::from_secs(5), key.as_ref())?;
        let initial = client.read_wave_camera_status()?;
        ensure!(
            initial["connected"] == true,
            "Camera live test requires a paired camera"
        );
        let original_mode = initial["mode"].as_u64().unwrap() as u8;
        let result = (|| -> Result<()> {
            client.request_wave_command(76, &[4])?;
            std::thread::sleep(Duration::from_secs(6));
            ensure!(
                client.read_wave_camera_status()?["mode"] == 4,
                "Camera did not enter desk view"
            );
            client.request_wave_command(76, &[0])?;
            std::thread::sleep(Duration::from_secs(6));
            ensure!(
                client.read_wave_camera_status()?["mode"] == 0,
                "Camera off reverted after desk view"
            );
            Ok(())
        })();
        // Restore even after a failed assertion, and leave an originally idle camera idle.
        if original_mode == 0 {
            client.request_wave_command(76, &[1])?;
        }
        client.request_wave_command(76, &[original_mode])?;
        result
    }

    #[test]
    fn resend_requests_follow_each_gap_without_repeating_for_every_report() -> Result<()> {
        let mut stream = WaveHidStream {
            last_received_id: Some(0),
            ..Default::default()
        };
        let receive = |stream: &mut WaveHidStream, id| {
            stream.receive_report(
                &WaveHidReport {
                    kind: 0,
                    packet_id: id,
                    payload: Vec::new(),
                }
                .encode_report()?,
            )
        };
        assert_eq!(receive(&mut stream, 3)?.outgoing.len(), 1);
        assert!(receive(&mut stream, 4)?.outgoing.is_empty());
        let next = receive(&mut stream, 1)?;
        assert_eq!(
            WaveHidReport::decode_report(&next.outgoing[0])?,
            WaveHidReport::control(1, 2)
        );
        assert!(
            stream
                .receive_report(&WaveHidReport::control(2, 2).encode_report()?)
                .is_err()
        );
        assert!(receive(&mut stream, 2)?.outgoing.is_empty());
        assert_eq!(stream.last_received_id, Some(4));
        Ok(())
    }

    #[test]
    fn complete_settings_catalog_fits_reply_assembler_but_excess_is_rejected() -> Result<()> {
        let data = vec![0; 25606];
        let mut assembler = WaveReplyAssembler::default();
        let count = data.len().div_ceil(112);
        for (index, chunk) in data.chunks(112).enumerate() {
            let end = if index + 1 == count {
                1
            } else if index == 0 {
                2
            } else {
                0
            };
            assert_eq!(
                assembler.accept_reply(&WavePcPacket {
                    command: 51,
                    payload: chunk.to_vec(),
                    sequence: u32::MAX,
                    session: WAVE_SESSION_TOKEN,
                    flags: 1 | (end << 5)
                })?,
                index + 1 == count
            );
        }
        assert_eq!(assembler.payload, data);
        assembler.payload.resize(32768, 0);
        assert!(
            assembler
                .accept_reply(&WavePcPacket {
                    command: 51,
                    payload: vec![0],
                    sequence: u32::MAX,
                    session: WAVE_SESSION_TOKEN,
                    flags: 1
                })
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn large_reply_can_recover_a_gap_beyond_128_hid_reports() -> Result<()> {
        let mut stream = WaveHidStream {
            last_received_id: Some(0),
            ..Default::default()
        };
        let report = WaveHidReport {
            kind: 0,
            packet_id: 129,
            payload: Vec::new(),
        };
        let incoming = stream.receive_report(&report.encode_report()?)?;
        assert_eq!(
            WaveHidReport::decode_report(&incoming.outgoing[0])?,
            WaveHidReport::control(1, 1)
        );
        for id in 1..129 {
            stream.receive_report(
                &WaveHidReport {
                    kind: 0,
                    packet_id: id,
                    payload: Vec::new(),
                }
                .encode_report()?,
            )?;
        }
        assert_eq!(stream.last_received_id, Some(129));
        assert!(stream.received_pending.is_empty());
        Ok(())
    }

    #[test]
    fn missing_hid_reports_are_reordered_and_duplicates_suppressed() -> Result<()> {
        let mut stream = WaveHidStream {
            last_received_id: Some(0),
            ..Default::default()
        };
        let packet = WavePcPacket {
            command: 43,
            payload: vec![0],
            sequence: u32::MAX,
            session: WAVE_SESSION_TOKEN,
            flags: 0x25,
        };
        let frame = encode_wave_mavlink(1000, &packet.encode_packet(None)?, 0)?;
        let chunks: Vec<_> = frame.chunks(52).collect();
        let mut messages = Vec::new();
        for id in [2, 1, 1, 3] {
            let report = WaveHidReport {
                kind: 0,
                packet_id: id,
                payload: chunks[id as usize - 1].to_vec(),
            };
            let result = stream.receive_report(&report.encode_report()?)?;
            if id == 2 {
                assert_eq!(
                    WaveHidReport::decode_report(&result.outgoing[0])?,
                    WaveHidReport::control(1, 1)
                );
            }
            messages.extend(result.messages);
        }
        assert_eq!(messages.len(), 1);
        assert_eq!(
            WavePcPacket::decode_packet(&messages[0].1, None)?.payload,
            [0]
        );
        Ok(())
    }

    #[test]
    fn resend_sync_reset_and_short_heartbeat() -> Result<()> {
        let mut stream = WaveHidStream::default();
        let sent = stream.send_pc_packet(&[42; 128])?;
        let resend = stream.receive_report(&WaveHidReport::control(1, 1).encode_report()?)?;
        assert_eq!(resend.outgoing, [sent[0]]);
        let missing = stream.receive_report(&WaveHidReport::control(1, 999).encode_report()?)?;
        assert_eq!(WaveHidReport::decode_report(&missing.outgoing[0])?.kind, 2);
        stream.last_received_id = Some(8);
        let sync = stream.receive_report(&WaveHidReport::control(5, 8).encode_report()?)?;
        assert_eq!(WaveHidReport::decode_report(&sync.outgoing[0])?.kind, 6);
        let reset = stream.receive_report(&WaveHidReport::control(3, 0).encode_report()?)?;
        assert_eq!(WaveHidReport::decode_report(&reset.outgoing[0])?.kind, 4);
        assert!(stream.last_received_id.is_none());
        let mut heartbeat = [0; 64];
        heartbeat[..3].copy_from_slice(&[3, 0xed, 58]);
        for (index, byte) in heartbeat[3..61].iter_mut().enumerate() {
            *byte = index as u8;
        }
        let reply = stream.receive_report(&heartbeat)?.outgoing[0];
        heartbeat[1] = 0xef;
        assert_eq!(reply, heartbeat);
        let camera = stream.camera_heartbeat.take().unwrap();
        assert_eq!(camera["zoom"], u16::from_le_bytes([35, 36]));
        assert!(camera["extended_privacy"].is_null());
        assert!(stream.receive_report(&heartbeat)?.outgoing.is_empty());
        assert_eq!(stream.camera_heartbeat.take().unwrap(), camera);
        heartbeat[2] = 57;
        assert!(stream.receive_report(&heartbeat).is_err());
        assert!(stream.receive_report(&[3, 0]).is_err());
        Ok(())
    }

    #[test]
    fn encrypted_multipart_reply_reaches_status_decoder() -> Result<()> {
        // Synthetic test key and packed status; no real credentials or device IDs.
        let key = *b"0123456789abcdef";
        let mut status = [0; 318];
        status[151] = 40;
        status[152] = 60;
        let mut stream = WaveHidStream::default();
        let mut assembler = WaveReplyAssembler::default();
        let mut id = 0;
        let mut finished = false;
        for (index, chunk) in status.chunks(112).enumerate() {
            let end = if index == 2 {
                1
            } else if index == 0 {
                2
            } else {
                0
            };
            let packet = WavePcPacket {
                command: 64,
                payload: chunk.to_vec(),
                sequence: u32::MAX,
                session: WAVE_SESSION_TOKEN,
                flags: 0x15 | (end << 5),
            };
            let frame = encode_wave_mavlink(1000, &packet.encode_packet(Some(&key))?, index as u8)?;
            for payload in frame.chunks(52) {
                id += 1;
                let wire = WaveHidReport {
                    kind: 0,
                    packet_id: id,
                    payload: payload.to_vec(),
                }
                .encode_report()?;
                for (_, payload) in stream.receive_report(&wire)?.messages {
                    let reply = WavePcPacket::decode_packet(&payload, Some(&key))?;
                    finished = assembler.accept_reply(&reply)?;
                }
            }
        }
        assert!(finished);
        assert_eq!(decode_wave_status(&assembler.payload)?["mic"]["volume"], 60);
        let mut assembler = WaveReplyAssembler::default();
        let reply = WavePcPacket {
            command: 64,
            payload: vec![0],
            sequence: 0,
            session: WAVE_SESSION_TOKEN,
            flags: 1,
        };
        assert!(assembler.accept_reply(&reply).is_err());
        Ok(())
    }
}
