//! CLI command groups and their packed request builders. No USB or cloud side effects here.

use anyhow::{Context, Result, bail, ensure};
use clap::{Subcommand, ValueEnum};
use insta360_wave::{
    wave_command_protocol::{encode_wave_datetime, wave_text_field},
    wave_protocol::WaveBindingKey,
};
use serde_json::Value;
use std::{env, fs, path::PathBuf};

#[derive(Clone, Copy, ValueEnum)]
pub enum WaveSwitch {
    On,
    Off,
}
impl WaveSwitch {
    fn byte(self) -> u8 {
        u8::from(matches!(self, Self::On))
    }
}

#[derive(Subcommand)]
pub enum WaveWifiCommand {
    /// Start an asynchronous scan; read results with list
    Scan,
    /// Read up to 64 scanned networks
    List,
    /// Read connection state, signal strength, and network addresses
    Status,
    /// Read SSID and network addresses
    Info,
    /// Connect using WAVE_WIFI_PASSWORD (omitting it sends an empty password)
    Connect {
        ssid: String,
        /// Firmware encryption mode; use the value reported by wifi list
        #[arg(long)]
        encryption: u8,
    },
    /// Disconnect a network and optionally forget its saved password
    Disconnect {
        ssid: String,
        #[arg(long)]
        forget: bool,
    },
    /// Forget a saved network password
    Forget { ssid: String },
    /// Enable or disable Wi-Fi
    Enable { value: WaveSwitch },
    /// Enable or disable automatic connection
    AutoConnect { value: WaveSwitch },
}

#[derive(Subcommand)]
pub enum WaveMeetingCommand {
    /// Start a new meeting recording
    Start,
    /// Finish the current recording
    Stop,
    /// Pause the current recording
    Pause,
    /// Resume a paused recording
    Resume,
    /// Read current meeting URL and duration
    Status,
    /// Read a meeting's name, URL, and upload state by local numeric ID
    Info { id: u16 },
    /// Read current meeting metadata, omitting the device token
    Current,
    /// Add a common (0), task (1), or image (2) mark
    Mark {
        #[arg(value_parser = clap::value_parser!(u8).range(0..=2))]
        kind: u8,
    },
    /// Select recording (0) or transcript-only (1) mode
    Mode {
        #[arg(value_parser = clap::value_parser!(u8).range(0..=1))]
        value: u8,
    },
    /// Register an existing screenshot object with a meeting
    Screenshot { elapsed: u32, object_name: String },
    /// Report desktop meeting application state (firmware platform/status enums)
    PcStatus {
        #[arg(long)]
        platform: u8,
        #[arg(long)]
        status: u8,
        #[arg(long)]
        running: bool,
        #[arg(long)]
        input_wave: bool,
        #[arg(long)]
        output_wave: bool,
    },
}

#[derive(Subcommand)]
pub enum WaveSystemCommand {
    /// Read language code, country, and timezone
    Info,
    /// Read the complete language/country/timezone catalog
    Settings,
    /// Read the firmware's default settings
    Defaults,
    /// Read the available languages
    Languages,
    /// Read countries for a suggested language
    Countries {
        #[arg(long, default_value = "")]
        language: String,
    },
    /// Read timezones for a suggested language and country
    Timezones {
        #[arg(long, default_value = "")]
        language: String,
        #[arg(long)]
        country: String,
    },
    /// Read language, or set it to a value from the catalog
    Language { value: Option<String> },
    /// Read country, or set it to a value from the catalog
    Country { value: Option<String> },
    /// Read the current city
    City,
    /// Set a timezone from the catalog; system info reads the current value
    Timezone { value: String },
    /// Set local device time in YYYY-MM-DDTHH:MM:SS format
    Time { value: String },
    /// Enable or disable automatic time synchronization
    AutoTime { value: WaveSwitch },
    /// Set the firmware time-display mode (numeric wire value)
    TimeFormat { value: u8 },
    /// Set display mode, automatic sync, and local time together
    Datetime {
        value: String,
        #[arg(long)]
        format: u8,
        #[arg(long)]
        auto: WaveSwitch,
    },
    /// Enable or disable USB mass storage; this may disconnect the HID interface
    Storage { value: WaveSwitch },
    /// Reboot the Wave
    Reboot,
    /// Restore factory settings and erase device configuration
    Reset,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum WaveCameraZoom {
    Out,
    In,
}

#[derive(Subcommand)]
pub enum WaveCameraCommand {
    /// Read paired-camera connection, mode, zoom, and privacy without a binding key
    Status,
    /// Increase or decrease the paired camera zoom
    Zoom { direction: WaveCameraZoom },
    /// Reset the paired camera position
    Reset,
    /// Stop paired-camera movement (equivalent to move 0)
    Stop,
    /// Move the camera using wire direction 0=stop, 1=-pan, 2=+pan, 3=+tilt, 4=-tilt
    Move {
        #[arg(value_parser = clap::value_parser!(u8).range(0..=4))]
        direction: u8,
    },
    /// Set 0=off, 1=single tracking, 2=group tracking, 3=whiteboard, 4=desk view
    Mode {
        #[arg(value_parser = clap::value_parser!(u8).range(0..=4))]
        value: u8,
    },
}

#[derive(Subcommand)]
pub enum WaveAccountCommand {
    /// Read binding metadata, omitting keys and tokens
    Info,
    /// Begin the device-side binding challenge
    BindStart,
    /// Request on-device password verification, or poll its result
    Password { action: WavePasswordAction },
    /// Write a complete binding record from a private JSON file (see README)
    Bind { file: PathBuf },
    /// Authenticate a user ID with WAVE_BIND_KEY
    Auth { user_id: String },
    /// End device authentication
    Unauth,
    /// Unbind the device; optionally remove its user data
    Unbind {
        #[arg(long)]
        remove_user_data: bool,
    },
    /// Authorize unbinding using WAVE_DEVICE_TOKEN
    AuthorizeUnbind,
    /// Invoke the firmware's offline-unbind handler
    UnbindOffline,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum WavePasswordAction {
    Request,
    Poll,
}

#[derive(Subcommand)]
pub enum WaveOtaCommand {
    /// Read firmware update status, progress, and error code
    Status,
    /// Switch USB into the firmware's OTA mass-storage/CDC mode
    Storage,
    /// Start installation of a package already staged on the Wave
    Start { package_name: String },
    /// Acknowledge firmware completion; nonzero status requests leaving OTA mode
    Finish { status: u8 },
}

pub struct WavePreparedCommand {
    pub id: u16,
    pub payload: Vec<u8>,
}

fn command(id: u16, payload: Vec<u8>) -> WavePreparedCommand {
    WavePreparedCommand { id, payload }
}
fn empty(id: u16) -> WavePreparedCommand {
    command(id, Vec::new())
}
fn byte(id: u16, value: u8) -> WavePreparedCommand {
    command(id, vec![value])
}
fn text(id: u16, value: &str) -> Result<WavePreparedCommand> {
    Ok(command(id, wave_text_field(value, 64)?))
}

fn wave_secret_environment(name: &str, optional: bool) -> Result<String> {
    match env::var(name) {
        Ok(value) => Ok(value),
        Err(env::VarError::NotPresent) if optional => Ok(String::new()),
        Err(_) => bail!("Set {name} to a valid Unicode value"),
    }
}

impl WaveWifiCommand {
    pub fn prepare(&self) -> Result<WavePreparedCommand> {
        Ok(match self {
            Self::Scan => empty(23),
            Self::List => empty(24),
            Self::Status => empty(27),
            Self::Info => empty(145),
            Self::Connect { ssid, encryption } => {
                ensure!(!ssid.is_empty(), "Wave Wi-Fi SSID must not be empty");
                let mut payload = wave_text_field(ssid, 64)?;
                payload.extend(wave_text_field(
                    &wave_secret_environment("WAVE_WIFI_PASSWORD", true)?,
                    64,
                )?);
                payload.push(*encryption);
                command(25, payload)
            }
            Self::Disconnect { ssid, forget } => {
                let mut payload = wave_text_field(ssid, 64)?;
                payload.push(u8::from(*forget));
                command(26, payload)
            }
            Self::Forget { ssid } => text(28, ssid)?,
            Self::Enable { value } => byte(29, value.byte()),
            Self::AutoConnect { value } => byte(30, value.byte()),
        })
    }
}

impl WaveMeetingCommand {
    pub fn prepare(&self) -> Result<WavePreparedCommand> {
        Ok(match self {
            Self::Start => byte(31, 0),
            Self::Stop => byte(31, 1),
            Self::Pause => byte(31, 2),
            Self::Resume => byte(31, 3),
            Self::Status => empty(32),
            Self::Info { id } => command(35, id.to_le_bytes().to_vec()),
            Self::Current => empty(36),
            Self::Mark { kind } => byte(33, *kind),
            Self::Mode { value } => byte(34, *value),
            Self::Screenshot {
                elapsed,
                object_name,
            } => {
                let mut payload = elapsed.to_le_bytes().to_vec();
                payload.extend(wave_text_field(object_name, 256)?);
                command(37, payload)
            }
            Self::PcStatus {
                platform,
                status,
                running,
                input_wave,
                output_wave,
            } => command(
                72,
                vec![
                    *platform,
                    *status,
                    u8::from(*running),
                    u8::from(*input_wave),
                    u8::from(*output_wave),
                ],
            ),
        })
    }
}

impl WaveSystemCommand {
    pub fn prepare(&self) -> Result<WavePreparedCommand> {
        Ok(match self {
            Self::Info => empty(144),
            Self::Settings => empty(51),
            Self::Defaults => empty(52),
            Self::Languages => empty(45),
            Self::Countries { language } => text(46, language)?,
            Self::Timezones { language, country } => {
                let mut payload = wave_text_field(language, 64)?;
                payload.extend(wave_text_field(country, 64)?);
                command(47, payload)
            }
            Self::Language { value } => match value {
                Some(value) => text(58, value)?,
                None => empty(49),
            },
            Self::Country { value } => match value {
                Some(value) => text(59, value)?,
                None => empty(48),
            },
            Self::City => empty(50),
            Self::Timezone { value } => text(61, value)?,
            Self::Time { value } => command(60, encode_wave_datetime(value, None)?),
            Self::Datetime {
                value,
                format,
                auto,
            } => command(
                56,
                encode_wave_datetime(value, Some((*format, auto.byte() != 0)))?,
            ),
            Self::AutoTime { value } => byte(55, value.byte()),
            Self::TimeFormat { value } => byte(54, *value),
            Self::Storage { value } => byte(53, value.byte()),
            Self::Reboot => empty(63),
            Self::Reset => empty(62),
        })
    }
}

impl WaveCameraCommand {
    pub fn prepare(&self) -> Option<WavePreparedCommand> {
        Some(match self {
            Self::Status => return None,
            Self::Zoom { direction } => byte(73, u8::from(matches!(direction, WaveCameraZoom::In))),
            Self::Reset => byte(74, 0),
            Self::Stop => byte(75, 0),
            Self::Move { direction } => byte(75, *direction),
            Self::Mode { value } => byte(76, *value),
        })
    }
}

impl WaveAccountCommand {
    pub fn prepare(&self) -> Result<WavePreparedCommand> {
        Ok(match self {
            Self::Info => empty(22),
            Self::BindStart => empty(16),
            Self::Password { action } => {
                // The callback ignores the password field: verification happens on the Wave screen.
                let mut payload = vec![u8::from(matches!(action, WavePasswordAction::Poll))];
                payload.resize(33, 0);
                command(17, payload)
            }
            Self::Bind { file } => {
                let bytes = fs::read(file).context("Cannot read Wave binding record")?;
                ensure!(bytes.len() <= 16_384, "Wave binding record exceeds 16 KiB");
                let record: Value = serde_json::from_slice(&bytes)
                    .map_err(|_| anyhow::anyhow!("Wave binding record is not valid JSON"))?;
                command(18, encode_wave_binding_record(&record)?)
            }
            Self::Auth { user_id } => {
                let key =
                    WaveBindingKey::parse_key(wave_secret_environment("WAVE_BIND_KEY", false)?)?;
                let mut payload = wave_text_field(user_id, 16)?;
                payload.extend(wave_text_field(key.secret_text(), 64)?);
                command(19, payload)
            }
            Self::Unauth => empty(20),
            Self::Unbind { remove_user_data } => byte(21, u8::from(*remove_user_data)),
            Self::AuthorizeUnbind => command(
                1,
                wave_text_field(&wave_secret_environment("WAVE_DEVICE_TOKEN", false)?, 512)?,
            ),
            Self::UnbindOffline => empty(2),
        })
    }
}

fn encode_wave_binding_record(record: &Value) -> Result<Vec<u8>> {
    let object = record
        .as_object()
        .context("Wave binding record must be an object")?;
    let fields = [
        ("user_id", 0, 16),
        ("key", 16, 64),
        ("area", 80, 128),
        ("cloud_token", 208, 512),
        ("nickname", 720, 128),
        ("email", 848, 128),
        ("phone", 976, 32),
        ("membership_url", 1008, 128),
        ("storage_limit", 1160, 16),
        ("storage_used", 1176, 16),
        ("vip_level", 1200, 16),
    ];
    let numbers = [
        ("benefit_reset_time", 1136, 8),
        ("transcription_limit", 1144, 4),
        ("transcription_used", 1148, 4),
        ("llm_limit", 1152, 4),
        ("llm_used", 1156, 4),
        ("vip_expire_time", 1192, 8),
    ];
    for name in object.keys() {
        ensure!(
            fields.iter().any(|(field, _, _)| field == name)
                || numbers.iter().any(|(field, _, _)| field == name),
            "Wave binding record contains an unknown field"
        );
    }
    let mut payload = vec![0; 1216];
    for (name, offset, width) in fields {
        let value = object
            .get(name)
            .and_then(Value::as_str)
            .with_context(|| format!("Wave binding record requires string field {name}"))?;
        if name == "key" {
            WaveBindingKey::parse_key(value.to_owned())?;
        }
        payload[offset..offset + width].copy_from_slice(&wave_text_field(value, width)?);
    }
    for (name, offset, width) in numbers {
        let value = object.get(name).and_then(Value::as_u64).with_context(|| {
            format!("Wave binding record requires unsigned integer field {name}")
        })?;
        ensure!(
            width == 8 || value <= u64::from(u32::MAX),
            "Wave binding field {name} exceeds u32"
        );
        payload[offset..offset + width].copy_from_slice(&value.to_le_bytes()[..width]);
    }
    Ok(payload)
}

impl WaveOtaCommand {
    pub fn prepare(&self) -> Result<WavePreparedCommand> {
        Ok(match self {
            Self::Status => empty(69),
            Self::Storage => empty(67),
            Self::Finish { status } => byte(70, *status),
            Self::Start { package_name } => {
                ensure!(
                    !package_name.is_empty()
                        && package_name != "."
                        && package_name != ".."
                        && !package_name.contains(['/', '\\']),
                    "Wave OTA package must be a filename without directories"
                );
                // The recovered callback ignores ota_op and always starts the staged package.
                let mut payload = vec![0];
                payload.extend(wave_text_field(package_name, 128)?);
                command(68, payload)
            }
        })
    }
}

pub fn prepare_wave_factory_mode(value: WaveSwitch) -> WavePreparedCommand {
    command(128, vec![0x47, value.byte()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_match_packed_firmware_layouts() -> Result<()> {
        for (operation, value) in [
            (WaveMeetingCommand::Start, 0),
            (WaveMeetingCommand::Stop, 1),
            (WaveMeetingCommand::Pause, 2),
            (WaveMeetingCommand::Resume, 3),
        ] {
            let request = operation.prepare()?;
            assert_eq!(request.id, 31);
            assert_eq!(request.payload, [value]);
        }
        let screenshot = WaveMeetingCommand::Screenshot {
            elapsed: 0x01020304,
            object_name: "image.png".into(),
        }
        .prepare()?;
        assert_eq!(screenshot.payload.len(), 260);
        assert_eq!(&screenshot.payload[..4], &[4, 3, 2, 1]);
        let tz = WaveSystemCommand::Timezones {
            language: "en".into(),
            country: "US".into(),
        }
        .prepare()?;
        assert_eq!(tz.payload.len(), 128);
        assert_eq!(&tz.payload[64..66], b"US");
        assert_eq!(
            WaveCameraCommand::Zoom {
                direction: WaveCameraZoom::Out
            }
            .prepare()
            .unwrap()
            .payload,
            [0]
        );
        assert!(WaveCameraCommand::Status.prepare().is_none());
        let stop = WaveCameraCommand::Stop.prepare().unwrap();
        assert_eq!((stop.id, stop.payload), (75, vec![0]));
        assert!(
            WaveOtaCommand::Start {
                package_name: "../package.bin".into()
            }
            .prepare()
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn binding_record_is_complete_bounded_and_little_endian() -> Result<()> {
        let mut record = serde_json::json!({"user_id":"test", "key":"synthetic-test-key", "area":"test", "cloud_token":"test", "nickname":"", "email":"", "phone":"", "membership_url":"", "storage_limit":"", "storage_used":"", "vip_level":"", "benefit_reset_time":0x0102030405060708u64, "transcription_limit":123, "transcription_used":0, "llm_limit":0, "llm_used":0, "vip_expire_time":0});
        let payload = encode_wave_binding_record(&record)?;
        assert_eq!(payload.len(), 1216);
        assert_eq!(&payload[1136..1144], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(&payload[1144..1148], &123u32.to_le_bytes());
        record["llm_used"] = serde_json::json!(u64::MAX);
        assert!(encode_wave_binding_record(&record).is_err());
        Ok(())
    }
}
