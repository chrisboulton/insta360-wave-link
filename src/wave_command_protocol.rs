//! Packed replies for the registered Wave PC-Link handlers, recovered from DWARF.
//! Account keys, cloud tokens, and device tokens are deliberately omitted from JSON.

use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

/// Encode one fixed-width UTF-8 field; lengths are bytes, not characters.
pub fn wave_text_field(value: &str, width: usize) -> Result<Vec<u8>> {
    ensure!(
        value.len() <= width && !value.contains('\0'),
        "Wave text field exceeds {width} bytes or contains NUL"
    );
    let mut field = vec![0; width];
    field[..value.len()].copy_from_slice(value.as_bytes());
    Ok(field)
}

/// OTA status zero clears local state but its callback leaves the ACK byte undefined.
pub fn decode_wave_ota_finish_reply(status: u8, data: &[u8]) -> Result<Value> {
    ensure!(
        data.len() == 1,
        "Wave OTA completion reply requires one byte"
    );
    if status == 0 {
        Ok(json!({"acknowledged": true, "verified": false}))
    } else {
        decode_wave_command_reply(70, data)
    }
}

pub(crate) fn wave_reply_text(data: &[u8], offset: usize, width: usize) -> String {
    let field = &data[offset..offset + width];
    String::from_utf8_lossy(&field[..field.iter().position(|&b| b == 0).unwrap_or(width)])
        .into_owned()
}

fn wave_reply_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}

fn wave_reply_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn wave_string_list(
    data: &[u8],
    offset: usize,
    count: usize,
    capacity: usize,
) -> Result<Vec<String>> {
    ensure!(
        count <= capacity,
        "Wave list count exceeds its packed capacity"
    );
    Ok((0..count)
        .map(|index| wave_reply_text(data, offset + index * 64, 64))
        .collect())
}

/// Check the exact packed size and result code before accessing any reply fields.
/// Firmware enum values stay numeric unless their meanings have been established.
pub fn decode_wave_command_reply(command: u16, data: &[u8]) -> Result<Value> {
    let (size, result_offset) = match command {
        1
        | 2
        | 18
        | 20
        | 21
        | 23
        | 25
        | 28..=31
        | 33
        | 34
        | 37..=43
        | 53..=56
        | 59..=63
        | 67
        | 68
        | 70
        | 72 => (1, Some(0)),
        16 => (17, Some(0)),
        17 => (1, None), // UI password request/poll has its own three-state result.
        19 => (85, Some(0)),
        22 => (722, None),    // slave_get_bind_info leaves result uninitialized.
        73 | 76 => (1, None), // Zoom and mode callbacks never initialize their ACK byte.
        74 | 75 => (1, Some(0)), // These callbacks store boolean success, unlike other commands.
        24 => (4290, Some(0)),
        26 => (65, Some(64)),
        27 => (133, Some(132)),
        32 => (517, Some(0)),
        35 => (644, Some(643)),
        36 => (801, None),
        45 => (1281, None),
        46 => (16642, None),
        47 => (7682, None),
        48 | 49 | 58 => (65, Some(64)),
        50 => (65, Some(0)),
        51 => (25606, Some(25605)),
        52 => (193, Some(192)),
        69 => (4, Some(0)),
        128 => (2, Some(0)),
        144 => (133, Some(132)),
        145 => (129, Some(128)),
        _ => bail!("Wave command 0x{command:02x} has no recovered reply decoder"),
    };
    ensure!(
        data.len() == size,
        "Wave command 0x{command:02x} requires {size} reply bytes; received {}",
        data.len()
    );
    if let Some(offset) = result_offset {
        ensure!(
            data[offset] == u8::from(matches!(command, 74 | 75)),
            "Wave command 0x{command:02x} was rejected (result {})",
            data[offset]
        );
    }
    let text = |offset, width| wave_reply_text(data, offset, width);
    Ok(match command {
        16 => {
            json!({"challenge": data[1..].iter().map(|b| format!("{b:02x}")).collect::<String>()})
        }
        17 => {
            json!({"result": data[0], "verification": match data[0] {0 => "pending", 1 => "verified", 2 => "failed", _ => "unknown"}})
        }
        19 => json!({"authenticated": true, "user_id": text(5, 16)}),
        22 => {
            ensure!(
                data[1] <= 1,
                "Wave binding status is invalid; check the key and firmware compatibility"
            );
            json!({"bound": data[1] != 0, "user_id": text(2, 16), "area": text(82, 128)})
        }
        73 | 76 => json!({"acknowledged": true, "verified": false}),
        24 => {
            ensure!(data[1] <= 64, "Wave Wi-Fi list count exceeds 64");
            let networks: Vec<_> = (0..usize::from(data[1]))
                .map(|index| {
                    let offset = 2 + index * 67;
                    json!({"ssid": text(offset, 64), "rssi": data[offset + 64] as i8,
                    "encryption": data[offset + 65], "password_saved": data[offset + 66] != 0})
                })
                .collect();
            json!({"networks": networks})
        }
        26 => json!({"ssid": text(0, 64), "acknowledged": true}),
        27 => json!({"ssid": text(0, 64), "mac": text(64, 32), "ipv4": text(96, 32),
            "rssi": data[128] as i8, "encryption": data[129], "status": data[130], "enabled": data[131] != 0}),
        32 => json!({"url": text(1, 512), "duration": wave_reply_u32(data, 513)}),
        35 => {
            json!({"id": wave_reply_u16(data, 0), "url": text(2, 512), "name": text(514, 128), "upload_status": data[642]})
        }
        36 => json!({"device_id": text(0, 16), "device_type": text(16, 8),
            "datetime": u64::from_le_bytes(data[24..32].try_into().unwrap()), "transcript_only": data[32] != 0,
            "language": text(33, 64), "local_meeting_id": text(97, 128), "timezone": text(225, 64)}),
        45 => json!({"languages": wave_string_list(data, 1, usize::from(data[0]), 20)?}),
        46 => {
            json!({"countries": wave_string_list(data, 2, usize::from(wave_reply_u16(data, 0)), 260)?})
        }
        47 => {
            json!({"timezones": wave_string_list(data, 2, usize::from(wave_reply_u16(data, 0)), 120)?})
        }
        48 => json!({"country": text(0, 64)}),
        49 | 58 => json!({"language": text(0, 64)}),
        50 => json!({"city": text(1, 64)}),
        51 => json!({"languages": wave_string_list(data, 1, usize::from(data[0]), 20)?,
            "countries": wave_string_list(data, 1283, usize::from(wave_reply_u16(data, 1281)), 260)?,
            "timezones": wave_string_list(data, 17925, usize::from(wave_reply_u16(data, 17923)), 120)?}),
        52 => json!({"language": text(0, 64), "country": text(64, 64), "timezone": text(128, 64)}),
        69 => json!({"status": data[1], "percent": data[2], "error_code": data[3]}),
        128 => json!({"acknowledged": true, "parameter": data[1]}),
        144 => {
            json!({"language_code": text(0, 4), "country": text(4, 64), "timezone": text(68, 64)})
        }
        145 => json!({"ssid": text(0, 64), "mac": text(64, 32), "ipv4": text(96, 32)}),
        _ => json!({"acknowledged": true}),
    })
}

/// Encode the six-u32 time command or the compact nine-byte datetime preferences.
/// Input uses the device's local wall clock, without an implicit timezone conversion.
pub fn encode_wave_datetime(value: &str, preferences: Option<(u8, bool)>) -> Result<Vec<u8>> {
    ensure!(
        value.len() == 19 && value.is_ascii(),
        "Wave time must be YYYY-MM-DDTHH:MM:SS"
    );
    let bytes = value.as_bytes();
    ensure!(
        bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
            && bytes[16] == b':',
        "Wave time must be YYYY-MM-DDTHH:MM:SS"
    );
    let mut parts = [0u32; 6];
    for (index, (start, end)) in [(0, 4), (5, 7), (8, 10), (11, 13), (14, 16), (17, 19)]
        .into_iter()
        .enumerate()
    {
        ensure!(
            bytes[start..end].iter().all(u8::is_ascii_digit),
            "Wave time contains nonnumeric fields"
        );
        parts[index] = value[start..end].parse()?;
    }
    let [year, month, day, hour, minute, second] = parts;
    ensure!(
        (1970..=9999).contains(&year) && (1..=12).contains(&month),
        "Wave time has invalid year or month"
    );
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ][month as usize - 1];
    ensure!(
        (1..=days).contains(&day) && hour <= 23 && minute <= 59 && second <= 59,
        "Wave time has invalid day or time"
    );
    if let Some((mode, automatic)) = preferences {
        let mut data = vec![mode, u8::from(automatic)];
        data.extend_from_slice(&(year as u16).to_le_bytes());
        data.extend(parts[1..].iter().map(|&n| n as u8));
        Ok(data)
    } else {
        Ok(parts.into_iter().flat_map(u32::to_le_bytes).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_lists_validate_sizes_counts_and_offsets() -> Result<()> {
        let mut data = vec![0; 25606];
        data[0] = 1;
        data[1..3].copy_from_slice(b"en");
        data[1281..1283].copy_from_slice(&1u16.to_le_bytes());
        data[1283..1285].copy_from_slice(b"US");
        data[17923..17925].copy_from_slice(&1u16.to_le_bytes());
        data[17925..17928].copy_from_slice(b"UTC");
        assert_eq!(
            decode_wave_command_reply(51, &data)?,
            json!({"languages": ["en"], "countries": ["US"], "timezones": ["UTC"]})
        );
        data[1281..1283].copy_from_slice(&261u16.to_le_bytes());
        assert!(decode_wave_command_reply(51, &data).is_err());
        assert!(decode_wave_command_reply(51, &data[..25605]).is_err());
        let mut wifi = vec![0; 4290];
        wifi[1] = 1;
        wifi[2..5].copy_from_slice(b"net");
        wifi[66] = (-42i8) as u8;
        wifi[67] = 3;
        wifi[68] = 1;
        assert_eq!(
            decode_wave_command_reply(24, &wifi)?["networks"][0],
            json!({"ssid":"net", "rssi": -42, "encryption":3, "password_saved":true})
        );
        Ok(())
    }

    #[test]
    fn credential_replies_are_redacted_and_failures_rejected() -> Result<()> {
        let mut binding = vec![0; 722];
        binding[0] = 112; // The firmware does not initialize this byte.
        binding[1] = 1;
        binding[18..82].fill(b'X');
        binding[210..722].fill(b'Y');
        let result = decode_wave_command_reply(22, &binding)?.to_string();
        assert!(!result.contains('X') && !result.contains('Y'));
        assert_eq!(decode_wave_command_reply(73, &[0xfe])?["verified"], false);
        assert_eq!(decode_wave_ota_finish_reply(0, &[0xfe])?["verified"], false);
        assert!(decode_wave_ota_finish_reply(1, &[2]).is_err());
        assert!(decode_wave_command_reply(63, &[2]).is_err());
        assert_eq!(
            decode_wave_command_reply(17, &[1])?["verification"],
            "verified"
        );
        Ok(())
    }

    #[test]
    fn camera_replies_follow_callback_boolean_and_uninitialized_ack() -> Result<()> {
        for command in [74, 75] {
            assert_eq!(
                decode_wave_command_reply(command, &[1])?["acknowledged"],
                true
            );
            assert!(decode_wave_command_reply(command, &[0]).is_err());
            assert!(decode_wave_command_reply(command, &[255]).is_err());
        }
        for command in [73, 76] {
            assert_eq!(
                decode_wave_command_reply(command, &[255])?["verified"],
                false
            );
            assert!(decode_wave_command_reply(command, &[]).is_err());
        }
        Ok(())
    }

    #[test]
    fn datetime_and_text_encoding_validate_bytes_and_calendar() -> Result<()> {
        let compact = encode_wave_datetime("2024-02-29T23:59:58", Some((1, false)))?;
        assert_eq!(compact, [1, 0, 0xe8, 7, 2, 29, 23, 59, 58]);
        assert_eq!(encode_wave_datetime("2024-02-29T23:59:58", None)?.len(), 24);
        for bad in [
            "2023-02-29T00:00:00",
            "2024-04-31T00:00:00",
            "2024-01-01T24:00:00",
            "２０２４-01-01T00:00:00",
        ] {
            assert!(encode_wave_datetime(bad, None).is_err());
        }
        assert!(wave_text_field("é", 1).is_err());
        assert!(wave_text_field("a\0b", 8).is_err());
        Ok(())
    }
}
