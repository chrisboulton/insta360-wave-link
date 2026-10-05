//! Wire formats recovered from libins_entry.so; multibyte integers are little endian.

use aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit},
};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub(crate) const WAVE_VENDOR_ID: u16 = 0x18f0;
pub(crate) const WAVE_PRODUCT_ID: u16 = 0x4e40;
pub(crate) const WAVE_SESSION_TOKEN: u32 = 0x4748494a;
pub(crate) const WAVE_PC_COMMAND_ID: u32 = 1000;

/// A binding key is a secret ASCII string; it deliberately has no Debug implementation.
pub struct WaveBindingKey(String);

impl WaveBindingKey {
    /// Validate a binding key without exposing its value in an error.
    pub fn parse_key(value: String) -> Result<Self> {
        ensure!(
            (16..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_graphic()),
            "Wave binding key must contain 16 to 64 printable ASCII characters"
        );
        Ok(Self(value))
    }

    /// Expose the binding key only for the explicit key-retrieval command's stdout.
    pub fn secret_text(&self) -> &str {
        &self.0
    }

    pub(crate) fn encryption_key(&self) -> [u8; 16] {
        self.0.as_bytes()[..16].try_into().unwrap()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WaveHidReport {
    pub kind: u8,
    pub packet_id: u64,
    pub payload: Vec<u8>,
}

impl WaveHidReport {
    pub fn control(kind: u8, packet_id: u64) -> Self {
        Self {
            kind,
            packet_id,
            payload: Vec::new(),
        }
    }

    pub fn encode_report(&self) -> Result<[u8; 64]> {
        ensure!(
            self.payload.len() <= 52,
            "Wave HID payload exceeds 52 bytes"
        );
        let mut wire = [0; 64];
        wire[0] = 3;
        wire[1] = self.kind;
        wire[2..10].copy_from_slice(&self.packet_id.to_le_bytes());
        wire[10] = self.payload.len() as u8;
        wire[11..11 + self.payload.len()].copy_from_slice(&self.payload);
        Ok(wire)
    }

    pub fn decode_report(wire: &[u8]) -> Result<Self> {
        ensure!(
            wire.len() == 64 && wire[0] == 3 && wire[10] <= 52,
            "Wave HID report has invalid ID, size, or payload length"
        );
        Ok(Self {
            kind: wire[1],
            packet_id: u64::from_le_bytes(wire[2..10].try_into().unwrap()),
            payload: wire[11..11 + usize::from(wire[10])].to_vec(),
        })
    }
}

fn wave_crc_extra(message_id: u32) -> Option<u8> {
    match message_id {
        0 => Some(69),
        1 => Some(32),
        1000 => Some(248),
        _ => None,
    }
}

fn wave_crc16(data: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for &value in data {
        let mut tmp = value ^ crc as u8;
        tmp ^= tmp << 4;
        let tmp = u16::from(tmp);
        crc = (crc >> 8) ^ (tmp << 8) ^ (tmp << 3) ^ (tmp >> 4);
    }
    crc
}

pub(crate) fn encode_wave_mavlink(
    message_id: u32,
    payload: &[u8],
    sequence: u8,
) -> Result<Vec<u8>> {
    let extra = wave_crc_extra(message_id)
        .ok_or_else(|| anyhow::anyhow!("Wave MAVLink message ID is unsupported"))?;
    ensure!(
        (1..=255).contains(&payload.len()),
        "Wave MAVLink payload length is unsupported"
    );
    let size = payload
        .iter()
        .rposition(|&b| b != 0)
        .map_or(1, |index| index + 1);
    let mut frame = vec![0xfd, size as u8, 0, 0, sequence, 0, 0];
    frame.extend_from_slice(&message_id.to_le_bytes()[..3]);
    frame.extend_from_slice(&payload[..size]);
    let mut checked = frame[1..].to_vec();
    checked.push(extra);
    frame.extend_from_slice(&wave_crc16(&checked).to_le_bytes());
    Ok(frame)
}

#[derive(Default)]
pub(crate) struct WaveMavlinkDecoder {
    buffer: Vec<u8>,
}

impl WaveMavlinkDecoder {
    pub fn feed_payload(&mut self, payload: &[u8]) -> Result<Vec<(u32, Vec<u8>)>> {
        self.buffer.extend_from_slice(payload);
        let mut messages = Vec::new();
        while !self.buffer.is_empty() {
            if self.buffer[0] != 0xfd {
                self.buffer.remove(0);
                continue;
            }
            if self.buffer.len() < 10 {
                break;
            }
            let size = usize::from(self.buffer[1]);
            let flags = self.buffer[2];
            ensure!(
                flags & !1 == 0,
                "Wave MAVLink frame has unsupported incompatibility flags"
            );
            let total = size + 12 + if flags & 1 != 0 { 13 } else { 0 };
            if self.buffer.len() < total {
                break;
            }
            let frame: Vec<_> = self.buffer.drain(..total).collect();
            let message_id = u32::from_le_bytes([frame[7], frame[8], frame[9], 0]);
            let Some(extra) = wave_crc_extra(message_id) else {
                continue;
            };
            let mut checked = frame[1..10 + size].to_vec();
            checked.push(extra);
            ensure!(
                wave_crc16(&checked) == u16::from_le_bytes([frame[10 + size], frame[11 + size]]),
                "Wave MAVLink checksum mismatch"
            );
            ensure!(flags == 0, "Wave MAVLink signed messages are unsupported");
            messages.push((message_id, frame[10..10 + size].to_vec()));
        }
        Ok(messages)
    }
}

pub(crate) struct WavePcPacket {
    pub command: u16,
    pub payload: Vec<u8>,
    pub sequence: u32,
    pub session: u32,
    pub flags: u8,
}

pub(crate) fn wave_command_is_encrypted(command: u16) -> bool {
    command > 16 && ![71, 128].contains(&command)
}

/// Split a logical command into PC-Link packets, advancing the sequence per packet.
pub(crate) fn encode_wave_command_packets(
    command: u16,
    payload: &[u8],
    sequence: &mut u32,
    session: u32,
    key: Option<&[u8; 16]>,
) -> Result<Vec<[u8; 128]>> {
    ensure!(
        payload.len() <= 32_768,
        "Wave command exceeds supported request size"
    );
    let count = payload.len().div_ceil(112).max(1);
    (0..count)
        .map(|index| {
            let start = index * 112;
            let end = (start + 112).min(payload.len());
            let end_flag = if index + 1 == count {
                1
            } else if index == 0 {
                2
            } else {
                0
            };
            let packet = WavePcPacket {
                command,
                payload: payload[start..end].to_vec(),
                sequence: *sequence,
                session,
                flags: 4 | (end_flag << 5),
            };
            *sequence = sequence.wrapping_add(1);
            packet.encode_packet(key)
        })
        .collect()
}

impl WavePcPacket {
    pub fn encode_packet(&self, key: Option<&[u8; 16]>) -> Result<[u8; 128]> {
        ensure!(
            self.payload.len() <= 112,
            "Wave PC-Link payload exceeds 112 bytes"
        );
        let mut wire = [0xff; 128];
        wire[..4].copy_from_slice(&self.session.to_le_bytes());
        wire[4] = 1;
        wire[5] = self.flags;
        wire[6..8].copy_from_slice(&self.command.to_le_bytes());
        wire[8..12].copy_from_slice(&self.sequence.to_le_bytes());
        // Empty queries still require the full FF padding block on the wire.
        let size = if self.payload.is_empty() {
            112
        } else {
            self.payload.len()
        };
        wire[12..16].copy_from_slice(&(size as u32).to_le_bytes());
        wire[16..16 + self.payload.len()].copy_from_slice(&self.payload);
        if let Some(key) = key.filter(|_| wave_command_is_encrypted(self.command)) {
            wire[5] |= 0x10;
            crypt_wave_payload(&mut wire[16..], key, false);
        }
        Ok(wire)
    }

    pub fn decode_packet(payload: &[u8], key: Option<&[u8; 16]>) -> Result<Self> {
        ensure!(
            payload.len() <= 128,
            "Wave PC-Link packet exceeds 128 bytes"
        );
        let mut wire = [0; 128];
        wire[..payload.len()].copy_from_slice(payload);
        let command = u16::from_le_bytes(wire[6..8].try_into().unwrap());
        let size = u32::from_le_bytes(wire[12..16].try_into().unwrap()) as usize;
        ensure!(
            wire[4] == 1 && size <= 112,
            "Wave PC-Link packet has invalid version or payload length"
        );
        if wire[5] & 0x10 != 0 && wave_command_is_encrypted(command) {
            let key = key.ok_or_else(|| {
                anyhow::anyhow!("Wave PC-Link encrypted packet requires a binding key")
            })?;
            crypt_wave_payload(&mut wire[16..], key, true);
        }
        Ok(Self {
            command,
            payload: wire[16..16 + size].to_vec(),
            sequence: u32::from_le_bytes(wire[8..12].try_into().unwrap()),
            session: u32::from_le_bytes(wire[..4].try_into().unwrap()),
            flags: wire[5],
        })
    }
}

fn crypt_wave_payload(payload: &mut [u8], key: &[u8; 16], decrypt: bool) {
    let cipher = Aes128::new(key.into());
    for block in payload.chunks_exact_mut(16) {
        if decrypt {
            cipher.decrypt_block(block.into());
        } else {
            cipher.encrypt_block(block.into());
        }
    }
}

/// Decode device info while excluding all binding credentials and cloud tokens.
pub fn decode_wave_info(payload: &[u8]) -> Result<Value> {
    ensure!(
        payload.len() == 933,
        "Wave device info requires 933 bytes; received {}",
        payload.len()
    );
    let text = |offset: usize, size: usize| {
        let bytes = &payload[offset..offset + size];
        String::from_utf8_lossy(&bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(size)])
            .into_owned()
    };
    let versions: Vec<_> = [(549, 32), (581, 32), (613, 32), (645, 32), (677, 31)]
        .into_iter()
        .map(|(offset, size)| text(offset, size))
        .collect();
    Ok(json!({"serial": text(0, 16), "bound": payload[16] != 0,
        "initialized": payload[547] != 0, "wifi_connected": payload[548] != 0,
        "firmware_versions": versions, "audio_model_type": payload[708],
        "model_version": text(709, 32), "model": text(741, 32), "usb_id": text(901, 32)}))
}

/// Decode audio, battery, Wi-Fi, and camera status; firmware enums remain numeric.
pub fn decode_wave_status(payload: &[u8]) -> Result<Value> {
    ensure!(
        payload.len() == 318,
        "Wave device status requires 318 bytes; received {}",
        payload.len()
    );
    if [payload[151], payload[152], payload[164]]
        .iter()
        .any(|&value| value > 100)
    {
        bail!("Wave status values are invalid; check the binding key and firmware compatibility");
    }
    let text =
        |offset, width| crate::wave_command_protocol::wave_reply_text(payload, offset, width);
    let u32_at =
        |offset: usize| u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap());
    let peripheral =
        |offset: usize| json!({"state": payload[offset] & 3, "error_code": payload[offset] >> 2});
    Ok(
        json!({"status": payload[0] & 15, "error_code": payload[0] >> 4,
        "bound": payload[1] & 1 != 0, "initialized": payload[1] & 2 != 0,
        "password_set": payload[1] & 4 != 0, "camera_connected": null,
        "storage_enabled": payload[1] & 8 != 0, "motor_level": (payload[1] >> 5) & 3,
        "wifi": {"enabled": payload[149] != 0, "status": payload[148], "ssid": text(18,64),
            "mac": text(82,32), "ipv4": text(114,32), "rssi": payload[146] as i8, "encryption": payload[147]},
        "speaker": {"volume": payload[151], "muted": payload[150] & 64 != 0, "mode": payload[154] & 15},
        "mic": {"volume": payload[152], "muted": payload[150] & 128 != 0, "mode": payload[154] >> 4},
        "audio": {"voice_db": payload[153],
            "sources": {"uac": payload[150] & 1 != 0, "bluetooth": payload[150] & 4 != 0, "dongle": payload[150] & 16 != 0},
            "mix": {"uac": payload[150] & 2 != 0, "bluetooth": payload[150] & 8 != 0, "dongle": payload[150] & 32 != 0},
            "inputs": {"uac": payload[155] & 1 != 0, "bluetooth": payload[155] & 2 != 0, "dongle": payload[155] & 4 != 0}},
        "battery": {"state": payload[163] & 3, "error_code": payload[163] >> 2, "level": payload[164]},
        "bluetooth": peripheral(165), "dongle": peripheral(166), "uac": peripheral(167),
        // On the wire, meet_status spans byte 155's high bit and byte 156's low two bits.
        // The desktop controller repacks this field before its GetMeetingStatus accessor.
        "meeting": {"status": (payload[155] >> 7) | ((payload[156] & 3) << 1),
            "duration": u32_at(157), "enabled": payload[161] & 1 != 0, "type": payload[162],
            "asr_local": payload[156] & 8 != 0, "asr_cloud": payload[156] & 16 != 0, "mode": (payload[156] >> 5) & 3,
            "upload_id": u16::from_le_bytes([payload[168],payload[169]]), "upload_status": payload[170]},
        "storage": {"total": u32_at(171), "used": u32_at(175)},
        "system": {"auto_time": payload[179] != 0, "time_format": payload[180],
            "language_code": text(181,4), "country": text(185,64), "timezone": text(249,64)},
        "ota": {"status": payload[313], "error_code": payload[314], "percent": payload[315]},
        "wifi_ui": payload[316], "user_improvement_plan": payload[317] != 0,
        // slave_get_dev_status initializes only the low two camera state bits.
        // Camera settings and connection must be read from the short heartbeat instead.
        "camera": {"state": payload[3] & 3, "error_code": null, "mode": null,
            "zoom": null, "privacy": null, "extended_privacy": null}}),
    )
}

/// Decode camera status from the 58-byte short heartbeat; omit the camera UUID.
/// Byte 34 is a UUID color flag despite the DWARF field's extended-privacy name.
pub fn decode_wave_camera_heartbeat(payload: &[u8]) -> Result<Value> {
    ensure!(
        payload.len() == 58,
        "Wave camera heartbeat requires 58 bytes"
    );
    Ok(json!({"connected": payload[2] == 1, "state": payload[2],
        "mode": payload[32], "zoom": u16::from_le_bytes([payload[35],payload[36]]),
        "privacy": payload[33] != 0, "extended_privacy": null,
        "source": "short_heartbeat"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_requests_use_per_packet_sequences_and_end_flags() -> Result<()> {
        let payload: Vec<_> = (0..1216).map(|index| (index % 251) as u8).collect();
        let key = *b"0123456789abcdef";
        let mut sequence = 7;
        let packets = encode_wave_command_packets(
            18,
            &payload,
            &mut sequence,
            WAVE_SESSION_TOKEN,
            Some(&key),
        )?;
        assert_eq!(packets.len(), 11);
        assert_eq!(sequence, 18);
        let mut assembled = Vec::new();
        for (index, wire) in packets.iter().enumerate() {
            let packet = WavePcPacket::decode_packet(wire, Some(&key))?;
            assert_eq!(packet.sequence, 7 + index as u32);
            assert_eq!(packet.flags & 3, 0);
            assert_eq!(
                (packet.flags >> 5) & 3,
                if index == 10 {
                    1
                } else if index == 0 {
                    2
                } else {
                    0
                }
            );
            assembled.extend(packet.payload);
        }
        assert_eq!(assembled, payload);
        let empty =
            encode_wave_command_packets(27, &[], &mut sequence, WAVE_SESSION_TOKEN, Some(&key))?;
        assert_eq!(
            WavePcPacket::decode_packet(&empty[0], Some(&key))?.payload,
            [0xff; 112]
        );
        Ok(())
    }

    #[test]
    fn hid_wire_layout_and_validation() -> Result<()> {
        let report = WaveHidReport {
            kind: 0,
            packet_id: 0x0102030405060708,
            payload: b"abc".to_vec(),
        };
        let mut wire = report.encode_report()?;
        assert_eq!(&wire[..14], &[3, 0, 8, 7, 6, 5, 4, 3, 2, 1, 3, 97, 98, 99]);
        assert_eq!(WaveHidReport::decode_report(&wire)?, report);
        assert!(WaveHidReport::decode_report(&wire[..63]).is_err());
        wire[10] = 53;
        assert!(WaveHidReport::decode_report(&wire).is_err());
        Ok(())
    }

    #[test]
    fn mavlink_checksum_fragmentation_and_zero_truncation() -> Result<()> {
        assert_eq!(wave_crc16(b"123456789"), 0x6f91);
        let mut payload = [0; 128];
        payload[..5].copy_from_slice(b"hello");
        let mut frame = encode_wave_mavlink(1000, &payload, 1)?;
        let mut decoder = WaveMavlinkDecoder::default();
        let mut messages = Vec::new();
        for byte in &frame {
            messages.extend(decoder.feed_payload(&[*byte])?);
        }
        assert_eq!(messages, vec![(1000, b"hello".to_vec())]);
        *frame.last_mut().unwrap() ^= 1;
        assert!(decoder.feed_payload(&frame).is_err());
        Ok(())
    }

    #[test]
    fn aes_matches_standard_vector_and_pc_link_plaintext_exceptions() -> Result<()> {
        let key = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let plain = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let mut body = plain.repeat(7);
        crypt_wave_payload(&mut body, &key, false);
        assert_eq!(
            &body[..16],
            &[
                0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4,
                0xc5, 0x5a
            ]
        );
        crypt_wave_payload(&mut body, &key, true);
        assert_eq!(body, plain.repeat(7));
        for command in [0, 16, 71, 128] {
            let packet = WavePcPacket {
                command,
                payload: vec![42],
                sequence: 0,
                session: WAVE_SESSION_TOKEN,
                flags: 0x35,
            };
            assert_eq!(
                WavePcPacket::decode_packet(&packet.encode_packet(Some(&key))?, None)?.payload,
                vec![42]
            );
        }
        let packet = WavePcPacket {
            command: 64,
            payload: Vec::new(),
            sequence: 0,
            session: WAVE_SESSION_TOKEN,
            flags: 0x24,
        };
        let wire = packet.encode_packet(None)?;
        assert_eq!(&wire[12..16], &112u32.to_le_bytes());
        assert_eq!(&wire[16..], &[0xff; 112]);
        assert!(WavePcPacket::decode_packet(&packet.encode_packet(Some(&key))?, None).is_err());
        assert_eq!(
            WavePcPacket::decode_packet(&packet.encode_packet(Some(&key))?, Some(&key))?.payload,
            vec![0xff; 112]
        );
        Ok(())
    }

    #[test]
    fn packed_device_data_omits_secrets_and_rejects_invalid_status() -> Result<()> {
        let mut info = [0; 933];
        info[..9].copy_from_slice(b"TEST-WAVE");
        info[34..50].copy_from_slice(b"synthetic-secret");
        assert!(
            !decode_wave_info(&info)?
                .to_string()
                .contains("synthetic-secret")
        );
        let mut status = [0; 318];
        status[1] = 16; // This camera connection bit is never written by the callback.
        status[3] = 0xfd; // Only low two state bits are initialized.
        status[4..11].fill(255);
        let camera_status = decode_wave_status(&status)?;
        assert!(camera_status["camera_connected"].is_null());
        assert_eq!(camera_status["camera"]["state"], 1);
        assert!(camera_status["camera"]["error_code"].is_null());
        assert!(camera_status["camera"]["zoom"].is_null());
        status[150] = 128;
        status[151] = 40;
        status[152] = 60;
        assert_eq!(decode_wave_status(&status)?["mic"]["muted"], true);
        status[151] = 250;
        assert!(decode_wave_status(&status).is_err());
        assert!(WaveBindingKey::parse_key("too-short".into()).is_err());
        assert!(WaveBindingKey::parse_key("0123456789abcde\n".into()).is_err());
        Ok(())
    }

    #[test]
    fn recording_status_decodes_live_packed_state_across_two_bytes() -> Result<()> {
        // Sanitized command-64 readback from idle, recording, paused, and stopping.
        for (input_and_status, meeting_flags, expected_state, seconds) in [
            (4, 24, 0, 0u32),
            (132, 24, 1, 2),
            (4, 25, 2, 3),
            (4, 26, 4, 5),
        ] {
            let mut payload = [0; 318];
            payload[155] = input_and_status;
            payload[156] = meeting_flags;
            payload[157..161].copy_from_slice(&seconds.to_le_bytes());
            let meeting = decode_wave_status(&payload)?["meeting"].clone();
            assert_eq!(meeting["status"], expected_state);
            assert_eq!(meeting["duration"], seconds);
        }
        for input_flags in [0, 127] {
            for recording_state in 0..=7 {
                let mut payload = [0; 318];
                payload[155] = input_flags | ((recording_state & 1) << 7);
                payload[156] = 0xfc | (recording_state >> 1);
                assert_eq!(
                    decode_wave_status(&payload)?["meeting"]["status"],
                    recording_state
                );
            }
        }
        Ok(())
    }
}
