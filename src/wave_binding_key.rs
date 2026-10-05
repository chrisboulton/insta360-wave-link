//! Read-only binding-key lookup using a controller login or WAVE_LOGIN_TOKEN.

use crate::wave_protocol::WaveBindingKey;
use anyhow::{Result, bail, ensure};
use ini::Ini;
use serde_json::Value;
use std::{
    env,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

/// Default US cloud endpoint; other supported regions can be selected for a supplied login.
pub const WAVE_DEFAULT_ENDPOINT: &str = "https://service-met-va.insta360.com/summary-service";
const WAVE_CLOUD_HOSTS: [&str; 6] = [
    "service-met-va.insta360.com",
    "service-met-hz.insta360.com",
    "service-met-fra.insta360.com",
    "openapi-va.insta360.com",
    "openapi-hz.insta360.com",
    "openapi-fra.insta360.com",
];

fn wave_binding_lookup_url(endpoint: &str, device_serial: &str) -> Result<Url> {
    let mut url = Url::parse(endpoint.trim_end_matches('/'))
        .map_err(|_| anyhow::anyhow!("Wave binding lookup endpoint is invalid"))?;
    ensure!(
        url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|host| WAVE_CLOUD_HOSTS.contains(&host))
            && url.path() == "/summary-service"
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Wave binding lookup endpoint must be a supported Insta360 production HTTPS service"
    );
    ensure!(
        !device_serial.is_empty(),
        "Wave binding lookup device serial is empty"
    );
    url.set_path("/summary-service/deviceBinding/getOneDeviceBindInfo");
    url.query_pairs_mut().append_pair("deviceId", device_serial);
    Ok(url)
}

fn read_wave_controller_login(config_path: &Path) -> Result<(String, String)> {
    let config = Ini::load_from_file(config_path)
        .map_err(|_| anyhow::anyhow!("Wave Controller configuration is unreadable or invalid"))?;
    let endpoint = config
        .section(Some("%General"))
        .or_else(|| config.section(Some("General")))
        .and_then(|section| section.get("EndPoint"));
    let token = config
        .section(Some("Token"))
        .and_then(|section| section.get("PcToken"));
    match (endpoint, token) {
        (Some(endpoint), Some(token)) => Ok((endpoint.to_owned(), token.to_owned())),
        _ => bail!("Wave Controller configuration is missing EndPoint/PcToken"),
    }
}

fn find_wave_controller_config() -> Result<PathBuf> {
    let home = env::var_os("HOME")
        .ok_or_else(|| anyhow::anyhow!("Wave Controller config path needs --controller-config"))?;
    let support = PathBuf::from(home).join("Library/Application Support");
    [
        support.join("Insta360/Insta360 Wave Controller/startup.ini"),
        support.join("Insta360 Wave Controller/startup.ini"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| {
        anyhow::anyhow!(
            "Wave Controller login not found; use --controller-config or WAVE_LOGIN_TOKEN"
        )
    })
}

fn decode_wave_binding_response(body: &[u8]) -> Result<WaveBindingKey> {
    let result: Value = serde_json::from_slice(body)
        .map_err(|_| anyhow::anyhow!("Wave binding lookup returned invalid JSON"))?;
    ensure!(
        result["code"].as_i64() == Some(0),
        "Wave binding lookup failed; check the account and device binding"
    );
    let key = result["data"]["bindKey"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Wave binding lookup did not return a binding key"))?;
    WaveBindingKey::parse_key(key.to_owned())
}

fn request_wave_binding_key(agent: &ureq::Agent, url: &Url, token: &str) -> Result<WaveBindingKey> {
    ensure!(
        !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_graphic()),
        "Wave login token is missing or invalid; sign in again"
    );
    let mut response = agent
        .get(url.as_str())
        .header("X-User-Token", token)
        .header("Accept", "application/json")
        .call()
        .map_err(|error| {
            // Report the failure class, never HTTP headers, URLs, or response bodies.
            let reason = match error {
                ureq::Error::Timeout(ureq::Timeout::Connect) => {
                    "connection or TLS handshake timed out"
                }
                ureq::Error::Timeout(ureq::Timeout::Resolve) => "hostname lookup timed out",
                ureq::Error::Timeout(_) => "request timed out",
                ureq::Error::HostNotFound => "hostname could not be resolved",
                ureq::Error::Rustls(_) | ureq::Error::Tls(_) => {
                    "TLS verification or handshake failed"
                }
                ureq::Error::Io(ref error)
                    if error.kind() == std::io::ErrorKind::ConnectionRefused =>
                {
                    "connection refused"
                }
                ureq::Error::Io(_) => "connection I/O failed",
                _ => "HTTP connection failed",
            };
            anyhow::anyhow!("Wave binding lookup could not reach Insta360: {reason}")
        })?;
    let status = response.status().as_u16();
    ensure!(
        (200..300).contains(&status),
        "Wave binding lookup returned HTTP {status}; check the controller login (redirects are refused)"
    );
    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut body)
        .map_err(|_| anyhow::anyhow!("Wave binding lookup could not read its response"))?;
    ensure!(
        body.len() <= 1024 * 1024,
        "Wave binding lookup response exceeds supported size"
    );
    decode_wave_binding_response(&body)
}

/// Fetch an existing key into memory; never bind/unbind, modify settings, or print login tokens.
pub fn retrieve_wave_binding_key(
    device_serial: &str,
    controller_login: bool,
    controller_config: Option<&Path>,
    endpoint: Option<&str>,
) -> Result<WaveBindingKey> {
    let (endpoint, token) = if controller_login {
        let config_path = match controller_config {
            Some(path) => path.to_owned(),
            None => find_wave_controller_config()?,
        };
        read_wave_controller_login(&config_path)?
    } else {
        let token = env::var("WAVE_LOGIN_TOKEN").map_err(|_| {
            anyhow::anyhow!("Wave binding-key requires --controller-login or WAVE_LOGIN_TOKEN")
        })?;
        (endpoint.unwrap_or(WAVE_DEFAULT_ENDPOINT).to_owned(), token)
    };
    let url = wave_binding_lookup_url(&endpoint, device_serial)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    request_wave_binding_key(&agent, &url, &token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn cloud_host_validation_and_serial_encoding() -> Result<()> {
        let url = wave_binding_lookup_url(WAVE_DEFAULT_ENDPOINT, "TEST/SERIAL")?;
        assert!(url.as_str().ends_with("deviceId=TEST%2FSERIAL"));
        for endpoint in [
            "https://example.com/summary-service",
            "http://service-met-va.insta360.com/summary-service",
            "https://service-met-va.insta360.com:444/summary-service",
            "https://token@service-met-va.insta360.com/summary-service",
            "https://service-met-va.insta360.com/summary-service?extra=1",
            "https://service-met-va.insta360.com/summary-service#fragment",
        ] {
            assert!(wave_binding_lookup_url(endpoint, "TEST-WAVE").is_err());
        }
        Ok(())
    }

    #[test]
    fn qt_controller_config_is_read_without_modification() -> Result<()> {
        let source = "[%General]\nEndPoint=https://service-met-va.insta360.com/summary-service\n[Token]\nPcToken=synthetic-login-token\n";
        let path = env::temp_dir().join(format!("wave-controller-test-{}.ini", std::process::id()));
        fs::write(&path, source)?;
        let result = read_wave_controller_login(&path);
        let unchanged = fs::read_to_string(&path)?;
        fs::remove_file(&path)?;
        let (endpoint, token) = result?;
        assert_eq!(endpoint, WAVE_DEFAULT_ENDPOINT);
        assert_eq!(token, "synthetic-login-token");
        assert_eq!(unchanged, source);
        Ok(())
    }

    #[test]
    fn cloud_responses_never_leak_tokens_on_errors() -> Result<()> {
        assert_eq!(
            decode_wave_binding_response(br#"{"code":0,"data":{"bindKey":"0123456789abcdef"}}"#)?
                .secret_text(),
            "0123456789abcdef"
        );
        for body in [
            br#"{"code":1,"msg":"synthetic-private-token"}"#.as_slice(),
            b"synthetic-private-token",
            br#"{"code":0,"data":{"bindKey":"short"}}"#,
        ] {
            let error = decode_wave_binding_response(body)
                .err()
                .unwrap()
                .to_string();
            assert!(!error.contains("synthetic-private-token"));
            assert!(!error.contains("short"));
        }
        Ok(())
    }

    #[test]
    fn lookup_sends_token_only_in_header_and_refuses_redirect() -> Result<()> {
        for redirect in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let url = Url::parse(&format!(
                "http://{}/lookup?deviceId=TEST-WAVE",
                listener.local_addr()?
            ))?;
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    request.push_str(&line);
                }
                assert!(request.starts_with("GET /lookup?deviceId=TEST-WAVE HTTP/1.1"));
                assert!(
                    request
                        .to_lowercase()
                        .contains("x-user-token: synthetic-login-token")
                );
                if redirect {
                    write!(stream, "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                } else {
                    let body = r#"{"code":0,"data":{"bindKey":"0123456789abcdef"}}"#;
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                }
            });
            // Production validates HTTPS/host before using its HTTPS-only agent.
            let agent = ureq::Agent::config_builder()
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(5)))
                .build()
                .into();
            let result = request_wave_binding_key(&agent, &url, "synthetic-login-token");
            server.join().unwrap();
            if redirect {
                assert!(result.err().unwrap().to_string().contains("HTTP 302"));
            } else {
                assert_eq!(result?.secret_text(), "0123456789abcdef");
            }
        }
        Ok(())
    }
}
