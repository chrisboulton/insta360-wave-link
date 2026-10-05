//! Wave CLI: retrieve a key once, then run USB commands using WAVE_BIND_KEY.

mod wave_commands;
use wave_commands::*;

use anyhow::{Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use insta360_wave::{
    wave_binding_key::retrieve_wave_binding_key,
    wave_client::{WaveUsbClient, list_wave_devices},
    wave_command_protocol::{decode_wave_command_reply, decode_wave_ota_finish_reply},
    wave_protocol::WaveBindingKey,
};
use serde_json::{Value, json};
use std::{
    env,
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

#[derive(Parser)]
#[command(
    name = "wave",
    version,
    about = "Control an Insta360 Wave over USB HID"
)]
struct WaveCli {
    /// Select a Wave by its USB serial number
    #[arg(long, global = true)]
    serial: Option<String>,
    /// USB reply timeout in seconds (greater than 0, at most 60)
    #[arg(long, global = true, default_value = "5", value_parser = parse_wave_timeout)]
    timeout: Duration,
    /// Print compact JSON (binding-key always prints only the raw key)
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: WaveCommand,
}

#[derive(Subcommand)]
enum WaveCommand {
    /// List connected Wave HID interfaces without opening them
    Devices,
    /// Read device identity and firmware versions without credentials
    Info,
    /// Read audio, battery, Wi-Fi, and camera state over USB
    Status,
    /// Retrieve the existing binding key; stdout contains only the key
    #[command(
        after_help = "Use --controller-login or supply WAVE_LOGIN_TOKEN in the environment. This is the only command that contacts Insta360."
    )]
    BindingKey {
        /// Read the signed-in Wave Controller's login and cloud endpoint
        #[arg(long)]
        controller_login: bool,
        /// Wave Controller startup.ini path
        #[arg(long, requires = "controller_login")]
        controller_config: Option<PathBuf>,
        /// Regional HTTPS endpoint when using WAVE_LOGIN_TOKEN
        #[arg(long, conflicts_with = "controller_login")]
        endpoint: Option<String>,
    },
    /// Control microphone volume or mute over USB
    Mic {
        #[command(subcommand)]
        setting: WaveAudioSetting,
    },
    /// Control speaker volume or mute over USB
    Speaker {
        #[command(subcommand)]
        setting: WaveAudioSetting,
    },
    /// Scan, inspect, and configure Wi-Fi over USB
    Wifi {
        #[command(subcommand)]
        command: WaveWifiCommand,
    },
    /// Record meetings and inspect meeting metadata
    Meeting {
        #[command(subcommand)]
        command: WaveMeetingCommand,
    },
    /// Read and change system, locale, clock, and storage settings
    System {
        #[command(subcommand)]
        command: WaveSystemCommand,
    },
    /// Control a paired camera through the Wave
    Camera {
        #[command(subcommand)]
        command: WaveCameraCommand,
    },
    /// Device-side account binding and authentication (no cloud lookup)
    Account {
        #[command(subcommand)]
        command: WaveAccountCommand,
    },
    /// Inspect and operate firmware updates using staged packages
    Ota {
        #[command(subcommand)]
        command: WaveOtaCommand,
    },
    /// Enter or leave factory mode; entering changes USB mode and enables ADB
    FactoryMode { value: WaveSwitch },
}

#[derive(Subcommand)]
enum WaveAudioSetting {
    /// Set volume from 0 to 100
    Volume {
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        value: u8,
    },
    /// Enable or disable mute
    Mute { value: WaveMuteValue },
    /// Set the numeric firmware microphone-processing or speaker-EQ mode
    Mode { value: u8 },
}

#[derive(Clone, Copy, ValueEnum)]
enum WaveMuteValue {
    On,
    Off,
}

fn parse_wave_timeout(value: &str) -> std::result::Result<Duration, String> {
    let seconds: f64 = value
        .parse()
        .map_err(|_| "timeout must be a number of seconds".to_owned())?;
    if !seconds.is_finite() || seconds <= 0.0 || seconds > 60.0 {
        return Err("timeout must be greater than 0 and at most 60 seconds".into());
    }
    let duration = Duration::from_secs_f64(seconds);
    if duration.is_zero() {
        return Err("timeout must be at least one nanosecond".into());
    }
    Ok(duration)
}

fn print_wave_result(value: &Value, compact: bool) -> Result<()> {
    let mut stdout = io::stdout().lock();
    if compact {
        serde_json::to_writer(&mut stdout, value)?;
    } else {
        serde_json::to_writer_pretty(&mut stdout, value)?;
    }
    writeln!(stdout)?;
    Ok(())
}

fn run_wave_command(cli: &WaveCli) -> Result<()> {
    let serial = cli.serial.as_deref();
    if let WaveCommand::Devices = cli.command {
        let devices = list_wave_devices(serial)?;
        print_wave_result(&json!(devices), cli.json)?;
        ensure!(
            !devices.is_empty(),
            "No Wave found. Plug it into USB with a data-capable cable."
        );
        return Ok(());
    }
    if let WaveCommand::BindingKey {
        controller_login,
        controller_config,
        endpoint,
    } = &cli.command
    {
        ensure!(
            *controller_login || env::var_os("WAVE_LOGIN_TOKEN").is_some(),
            "Wave binding-key requires --controller-login or WAVE_LOGIN_TOKEN"
        );
        // Close USB before waiting on HTTPS; controllers can contend for the HID stream.
        let info = WaveUsbClient::open_device(serial, cli.timeout, None)?.read_wave_info()?;
        ensure!(
            info["bound"].as_bool() == Some(true),
            "Wave is unbound; USB controls do not require a binding key"
        );
        let device_serial = info["serial"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Wave device info has no serial number"))?;
        let key = retrieve_wave_binding_key(
            device_serial,
            *controller_login,
            controller_config.as_deref(),
            endpoint.as_deref(),
        )?;
        writeln!(io::stdout().lock(), "{}", key.secret_text())?;
        return Ok(());
    }
    let key = if matches!(
        cli.command,
        WaveCommand::Info
            | WaveCommand::Camera {
                command: WaveCameraCommand::Status
            }
    ) {
        None
    } else {
        match env::var("WAVE_BIND_KEY") {
            Ok(value) => Some(WaveBindingKey::parse_key(value)?),
            Err(env::VarError::NotPresent) => None,
            Err(_) => anyhow::bail!("Wave binding key environment value is not valid Unicode"),
        }
    };
    // Validate and build requests before opening USB, including private input files.
    let prepared = match &cli.command {
        WaveCommand::Wifi { command } => Some(command.prepare()?),
        WaveCommand::Meeting { command } => Some(command.prepare()?),
        WaveCommand::System { command } => Some(command.prepare()?),
        WaveCommand::Camera { command } => command.prepare(),
        WaveCommand::Account { command } => Some(command.prepare()?),
        WaveCommand::Ota { command } => Some(command.prepare()?),
        WaveCommand::FactoryMode { value } => Some(prepare_wave_factory_mode(*value)),
        _ => None,
    };
    let mut client = WaveUsbClient::open_device(serial, cli.timeout, key.as_ref())?;
    if let Some(request) = prepared {
        let reply = client.request_wave_command(request.id, &request.payload)?;
        let result = if request.id == 70 {
            decode_wave_ota_finish_reply(request.payload[0], &reply)?
        } else {
            decode_wave_command_reply(request.id, &reply)?
        };
        return print_wave_result(&result, cli.json);
    }
    let result = match &cli.command {
        WaveCommand::Info => client.read_wave_info()?,
        WaveCommand::Status => client.read_wave_status()?,
        WaveCommand::Camera {
            command: WaveCameraCommand::Status,
        } => client.read_wave_camera_status()?,
        WaveCommand::Mic { setting } | WaveCommand::Speaker { setting } => {
            let speaker = matches!(cli.command, WaveCommand::Speaker { .. });
            let (setting_name, command, value) = match setting {
                WaveAudioSetting::Volume { value } => {
                    ("volume", if speaker { 38 } else { 41 }, *value)
                }
                WaveAudioSetting::Mute { value } => (
                    "mute",
                    if speaker { 40 } else { 43 },
                    u8::from(matches!(value, WaveMuteValue::On)),
                ),
                WaveAudioSetting::Mode { value } => ("mode", if speaker { 39 } else { 42 }, *value),
            };
            decode_wave_command_reply(command, &client.request_wave_command(command, &[value])?)?;
            json!({"target": if speaker { "speaker" } else { "mic" }, "setting": setting_name, "value": value, "acknowledged": true})
        }
        _ => unreachable!(),
    };
    print_wave_result(&result, cli.json)
}

fn main() {
    let cli = WaveCli::parse();
    if let Err(error) = run_wave_command(&cli) {
        if cli.json {
            eprintln!("{}", json!({"error": error.to_string()}));
        } else {
            eprintln!("wave: {error}");
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_validates_values_and_keeps_login_flags_on_binding_key() {
        for arguments in [
            vec!["wave", "mic", "volume", "101"],
            vec!["wave", "--timeout", "nan", "info"],
            vec!["wave", "speaker", "mute", "maybe"],
            vec!["wave", "camera", "move", "5"],
            vec!["wave", "camera", "mode", "5"],
            vec!["wave", "meeting", "mark", "3"],
            vec!["wave", "system", "timezones"],
            vec!["wave", "status", "--controller-login"],
            vec!["wave", "binding-key", "--controller-config", "test.ini"],
            vec![
                "wave",
                "binding-key",
                "--controller-login",
                "--endpoint",
                "https://example.com",
            ],
        ] {
            assert!(WaveCli::try_parse_from(arguments).is_err());
        }
        assert!(WaveCli::try_parse_from(["wave", "binding-key", "--controller-login"]).is_ok());
        assert!(WaveCli::try_parse_from(["wave", "speaker", "volume", "60", "--json"]).is_ok());
        assert!(WaveCli::try_parse_from(["wave", "camera", "status"]).is_ok());
        assert!(WaveCli::try_parse_from(["wave", "camera", "stop"]).is_ok());
        for arguments in [
            vec!["wave", "wifi", "connect", "Example", "--encryption", "3"],
            vec![
                "wave",
                "meeting",
                "pc-status",
                "--platform",
                "1",
                "--status",
                "0",
            ],
            vec!["wave", "system", "timezones", "--country", "United States"],
            vec![
                "wave",
                "system",
                "datetime",
                "2024-02-29T23:59:58",
                "--format",
                "1",
                "--auto",
                "off",
            ],
            vec!["wave", "account", "password", "poll"],
            vec!["wave", "ota", "start", "package.swu"],
            vec!["wave", "factory-mode", "off"],
        ] {
            assert!(WaveCli::try_parse_from(arguments).is_ok());
        }
    }
}
