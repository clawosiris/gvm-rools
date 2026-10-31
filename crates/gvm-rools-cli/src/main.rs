// SPDX-License-Identifier: AGPL-3.0-or-later

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use gvm_connection::{
    GvmConnection, SshAuth, SshConfig, SshConnection, UnixSocketConfig, UnixSocketConnection,
};
use gvm_gmp::commands::authentication::AuthenticateRequest;
use gvm_gmp::responses::AuthenticateResponse;
use gvm_gmp::{GmpRequestCodec, GmpVersion};
use gvm_protocol::Response;
use quick_xml::events::Event;
use quick_xml::{Reader, Writer};
use zeroize::Zeroizing;

#[derive(Parser, Debug)]
#[command(name = "gvm-cli")]
#[command(about = "Rust reimplementation of gvm-tools gvm-cli (GMP only)", long_about = None)]
struct Cli {
    /// GMP username (optional; if provided, gvm-cli will authenticate before sending the command)
    #[arg(long, env = "GMP_USERNAME", global = true)]
    gmp_username: Option<String>,

    /// GMP password (optional; if omitted but username is provided, gvm-cli will prompt)
    #[arg(long, env = "GMP_PASSWORD", global = true)]
    gmp_password: Option<String>,

    /// XML request to send (if omitted, read from infile or stdin)
    #[arg(short = 'X', long, global = true)]
    xml: Option<String>,

    /// Return raw XML even for non-2xx responses (do not treat as error)
    #[arg(short = 'r', long, global = true)]
    raw: bool,

    /// Pretty format the returned XML
    #[arg(long, global = true)]
    pretty: bool,

    /// Measure request/response round-trip time (excludes connection and authentication)
    #[arg(long, global = true)]
    duration: bool,

    /// Print connection and protocol details to stderr
    #[arg(long, global = true)]
    verbose: bool,

    /// File to read XML commands from (if --xml not provided)
    infile: Option<PathBuf>,

    #[command(subcommand)]
    transport: Transport,
}

#[derive(Subcommand, Debug)]
enum Transport {
    /// Connect via Unix domain socket
    Socket {
        /// Path to gvmd socket
        #[arg(long, default_value = "/run/gvmd/gvmd.sock")]
        path: PathBuf,

        /// Timeout in seconds (use -1 for no timeout)
        #[arg(long, default_value_t = 60)]
        timeout: i64,
    },

    /// Connect via SSH direct-streamlocal tunnel to remote gvmd unix socket
    Ssh {
        #[arg(long)]
        hostname: String,

        #[arg(long, default_value_t = 22)]
        port: u16,

        #[arg(long, default_value = "gvm")]
        username: String,

        /// Password authentication (prefer SSH_PASSWORD or --password-prompt to avoid CLI exposure)
        #[arg(long, env = "SSH_PASSWORD", hide_env_values = true)]
        password: Option<String>,

        /// Prompt for the SSH password from a TTY instead of using SSH agent auth
        #[arg(long, conflicts_with = "password")]
        password_prompt: bool,

        /// Remote gvmd socket path
        #[arg(long, default_value = "/run/gvmd/gvmd.sock")]
        remote_socket: String,
    },

    /// TLS transport (not yet implemented in rust-gvm)
    Tls,
}

#[derive(Debug, PartialEq, Eq)]
enum PasswordResolution {
    None,
    Provided(String),
    Prompt(&'static str),
}

#[derive(Debug, PartialEq, Eq)]
enum RequiredPasswordResolution {
    Provided(String),
    Prompt(&'static str),
}

struct GmpCredentials {
    username: String,
    password: Zeroizing<String>,
}

const GMP_PROFILE: GmpVersion = GmpVersion(22, 8);

fn verbose_log(enabled: bool, message: impl AsRef<str>) {
    if enabled {
        eprintln!("[verbose] {}", message.as_ref());
    }
}

impl Transport {
    fn describe(&self) -> String {
        match self {
            Self::Socket { path, timeout } => {
                format!("unix socket path={} timeout={}s", path.display(), timeout)
            }
            Self::Ssh {
                hostname,
                port,
                username,
                password,
                password_prompt,
                remote_socket,
            } => {
                let auth = if password.is_some() {
                    "password"
                } else if *password_prompt {
                    "password-prompt"
                } else {
                    "ssh-agent"
                };
                format!(
                    "ssh host={hostname} port={port} user={username} remote_socket={remote_socket} auth={auth}"
                )
            }
            Self::Tls => "tls".to_string(),
        }
    }
}

async fn read_xml(cli: &Cli) -> Result<String> {
    if let Some(xml) = &cli.xml {
        return Ok(xml.clone());
    }

    if let Some(path) = &cli.infile {
        return std::fs::read_to_string(path)
            .with_context(|| format!("failed to read infile {}", path.display()));
    }

    // stdin
    tokio::task::spawn_blocking(read_stdin_to_string)
        .await
        .context("stdin read task failed")?
        .context("failed to read stdin")
}

fn read_stdin_to_string() -> std::io::Result<String> {
    let mut buf = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
    Ok(buf)
}

async fn authenticate_if_needed<C: GvmConnection + ?Sized>(
    conn: &mut C,
    credentials: Option<&GmpCredentials>,
    raw: bool,
) -> Result<()> {
    let Some(credentials) = credentials else {
        return Ok(());
    };

    let auth_command = Zeroizing::new(
        AuthenticateRequest::new(&credentials.username, credentials.password.as_str())
            .encode(GMP_PROFILE)
            .context("encode authenticate failed")?,
    );

    conn.send(auth_command.as_slice())
        .await
        .context("send authenticate failed")?;
    let resp_bytes = conn.read().await.context("read authenticate failed")?;
    let resp = Response::new(resp_bytes);

    if raw {
        return Ok(());
    }

    AuthenticateResponse::from_response(&resp)
        .map_err(|error| anyhow!("authentication failed: {error}"))?;

    Ok(())
}

async fn resolve_gmp_credentials(cli: &mut Cli) -> Result<Option<GmpCredentials>> {
    let Some(username) = cli.gmp_username.take() else {
        let _unused_password = cli.gmp_password.take().map(Zeroizing::new);
        return Ok(None);
    };

    let password = match resolve_required_gmp_password_with(
        cli.gmp_password.take(),
        std::io::stdin().is_terminal(),
    )? {
        RequiredPasswordResolution::Provided(password) => Zeroizing::new(password),
        RequiredPasswordResolution::Prompt(prompt) => {
            prompt_password(prompt, "failed to read GMP password from TTY").await?
        }
    };

    Ok(Some(GmpCredentials { username, password }))
}

fn resolve_required_gmp_password_with(
    password: Option<String>,
    stdin_is_terminal: bool,
) -> Result<RequiredPasswordResolution> {
    if let Some(password) = password {
        return Ok(RequiredPasswordResolution::Provided(password));
    }

    if stdin_is_terminal {
        return Ok(RequiredPasswordResolution::Prompt("GMP Password: "));
    }

    Err(anyhow!(
        "--gmp-password is required when --gmp-username is set"
    ))
}

async fn resolve_ssh_password(
    password: Option<String>,
    password_prompt: bool,
) -> Result<Option<Zeroizing<String>>> {
    match resolve_ssh_password_with(password, password_prompt, std::io::stdin().is_terminal())? {
        PasswordResolution::None => Ok(None),
        PasswordResolution::Provided(password) => Ok(Some(Zeroizing::new(password))),
        PasswordResolution::Prompt(prompt) => {
            prompt_password(prompt, "failed to read SSH password from TTY")
                .await
                .map(Some)
        }
    }
}

fn resolve_ssh_password_with(
    password: Option<String>,
    password_prompt: bool,
    stdin_is_terminal: bool,
) -> Result<PasswordResolution> {
    if let Some(password) = password {
        return Ok(PasswordResolution::Provided(password));
    }

    if password_prompt {
        if stdin_is_terminal {
            return Ok(PasswordResolution::Prompt("SSH Password: "));
        }

        return Err(anyhow!(
            "--password-prompt requires a TTY or use SSH_PASSWORD"
        ));
    }

    Ok(PasswordResolution::None)
}

async fn prompt_password(prompt: &'static str, context: &'static str) -> Result<Zeroizing<String>> {
    tokio::task::spawn_blocking(move || prompt_password_from_tty(prompt))
        .await
        .context("password prompt task failed")?
        .map(Zeroizing::new)
        .context(context)
}

fn prompt_password_from_tty(prompt: &str) -> std::io::Result<String> {
    rpassword::prompt_password(prompt)
}

fn socket_timeout(timeout: i64) -> Option<std::time::Duration> {
    if timeout < 0 {
        None
    } else {
        Some(std::time::Duration::from_secs(timeout as u64))
    }
}

fn format_xml(xml: &[u8], pretty: bool) -> Result<Vec<u8>> {
    if !pretty {
        return Ok(xml.to_vec());
    }

    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);

    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Eof) => break,
            Ok(event) => writer
                .write_event(event)
                .context("failed to pretty-print XML response")?,
            Err(error) => {
                return Err(anyhow!("failed to pretty-print XML response: {error}"));
            }
        }
        buffer.clear();
    }

    let formatted = writer.into_inner();
    std::str::from_utf8(&formatted).context("pretty-printed XML was not valid UTF-8")?;
    Ok(formatted)
}

fn write_stdout(response: &[u8]) -> Result<()> {
    std::io::stdout()
        .lock()
        .write_all(response)
        .context("failed to write response to stdout")
}

async fn run(mut cli: Cli) -> Result<i32> {
    let xml = read_xml(&cli).await?.trim().to_string();
    if xml.is_empty() {
        return Err(anyhow!("no XML provided (use --xml, infile, or stdin)"));
    }
    verbose_log(
        cli.verbose,
        format!("transport: {}", cli.transport.describe()),
    );
    verbose_log(cli.verbose, format!("request bytes: {}", xml.len()));
    verbose_log(cli.verbose, format!("request xml:\n{xml}"));
    let gmp_credentials = resolve_gmp_credentials(&mut cli).await?;

    let mut conn: Box<dyn GvmConnection> = match cli.transport {
        Transport::Socket { path, timeout } => {
            let mut cfg = UnixSocketConfig::new(path);
            if let Some(t) = socket_timeout(timeout) {
                cfg = cfg.with_timeout(t);
            }
            Box::new(UnixSocketConnection::new(cfg))
        }
        Transport::Ssh {
            hostname,
            port,
            username,
            password,
            password_prompt,
            remote_socket,
        } => {
            let auth = resolve_ssh_password(password, password_prompt)
                .await?
                .map(|password| SshAuth::Password(password.to_string()))
                .unwrap_or(SshAuth::Agent);
            let cfg = SshConfig::new(hostname, username, auth)
                .with_port(port)
                .with_remote_socket(remote_socket);
            Box::new(SshConnection::new(cfg))
        }
        Transport::Tls => {
            return Err(anyhow!(
                "TLS transport not implemented yet (see rust-gvm TLS transport issue)"
            ));
        }
    };

    verbose_log(cli.verbose, "connecting");
    conn.connect().await.context("connect failed")?;
    verbose_log(cli.verbose, "connected");

    let execution_result = async {
        if let Some(credentials) = &gmp_credentials {
            verbose_log(
                cli.verbose,
                format!("authenticating as GMP user {}", credentials.username),
            );
        }
        authenticate_if_needed(conn.as_mut(), gmp_credentials.as_ref(), cli.raw).await?;

        let start = Instant::now();
        verbose_log(cli.verbose, "sending request");
        conn.send(xml.as_bytes()).await.context("send failed")?;
        verbose_log(cli.verbose, "waiting for response");
        let resp_bytes = conn.read().await.context("read failed")?;
        let elapsed = start.elapsed();
        verbose_log(cli.verbose, format!("response bytes: {}", resp_bytes.len()));
        verbose_log(
            cli.verbose,
            format!("response xml:\n{}", String::from_utf8_lossy(&resp_bytes)),
        );
        verbose_log(
            cli.verbose,
            format!("round-trip time: {:.3} seconds", elapsed.as_secs_f64()),
        );

        let resp = Response::new(resp_bytes);

        if !cli.raw && !resp.is_success() {
            let status = resp.status_code().unwrap_or(0);
            let text = resp
                .status_text()
                .unwrap_or_else(|| "<no status text>".to_string());
            eprintln!("server rejected command (status {status}): {text}");
            // Still print the response to stdout for debugging.
            write_stdout(&format_xml(resp.as_ref(), cli.pretty)?)?;
            return Ok(1);
        }

        write_stdout(&format_xml(resp.as_ref(), cli.pretty)?)?;

        if cli.duration {
            eprintln!("Elapsed time: {:.3} seconds", elapsed.as_secs_f64());
        }

        Ok(0)
    }
    .await;

    verbose_log(cli.verbose, "disconnecting");
    conn.disconnect().await.ok();
    verbose_log(cli.verbose, "disconnected");

    execution_result
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("error: {err:#}");
            std::process::exit(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_xml, resolve_required_gmp_password_with, resolve_ssh_password_with, socket_timeout,
        Cli, PasswordResolution, RequiredPasswordResolution, Transport, GMP_PROFILE,
    };
    use clap::Parser;
    use gvm_gmp::commands::authentication::AuthenticateRequest;
    use gvm_gmp::GmpRequestCodec;
    use std::time::Duration;

    #[test]
    fn uses_provided_gmp_password_without_prompting() {
        let result = resolve_required_gmp_password_with(Some("secret".to_string()), false).unwrap();

        assert_eq!(
            result,
            RequiredPasswordResolution::Provided("secret".to_string())
        );
    }

    #[test]
    fn prompts_for_password_when_username_is_set_and_stdin_is_tty() {
        let result = resolve_required_gmp_password_with(None, true).unwrap();

        assert_eq!(result, RequiredPasswordResolution::Prompt("GMP Password: "));
    }

    #[test]
    fn returns_error_when_username_is_set_without_password_and_stdin_is_not_tty() {
        let error = resolve_required_gmp_password_with(None, false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "--gmp-password is required when --gmp-username is set"
        );
    }

    #[test]
    fn prefers_supplied_ssh_password_over_agent_auth() {
        let result =
            resolve_ssh_password_with(Some("ssh-secret".to_string()), false, false).unwrap();

        assert_eq!(
            result,
            PasswordResolution::Provided("ssh-secret".to_string())
        );
    }

    #[test]
    fn prompts_for_ssh_password_when_requested() {
        let result = resolve_ssh_password_with(None, true, true).unwrap();

        assert_eq!(result, PasswordResolution::Prompt("SSH Password: "));
    }

    #[test]
    fn errors_when_ssh_password_prompt_lacks_tty() {
        let error = resolve_ssh_password_with(None, true, false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "--password-prompt requires a TTY or use SSH_PASSWORD"
        );
    }

    #[test]
    fn escapes_special_characters_in_auth_xml() {
        let request = AuthenticateRequest::new("admin&<>'\"", "foo&bar<baz>'\"");
        let auth_xml = String::from_utf8(request.encode(GMP_PROFILE).expect("valid auth request"))
            .expect("auth xml utf-8");

        assert_eq!(
            auth_xml,
            "<authenticate><credentials><username>admin&amp;&lt;&gt;&apos;&quot;</username><password>foo&amp;bar&lt;baz&gt;&apos;&quot;</password></credentials></authenticate>"
        );
    }

    #[test]
    fn authentication_encoding_errors_redact_credentials() {
        let request = AuthenticateRequest::new("admin", "secret\0value");
        let error = request
            .encode(GMP_PROFILE)
            .expect_err("invalid XML character must be rejected");
        let message = error.to_string();

        assert!(message.contains("password"));
        assert!(!message.contains("secret"));
        assert!(!format!("{request:?}").contains("secret"));
    }

    #[test]
    fn pretty_prints_xml_with_indentation() {
        let formatted = format_xml(b"<root><child>value</child></root>", true).unwrap();
        assert!(String::from_utf8(formatted)
            .unwrap()
            .contains("\n  <child>value</child>\n"));
    }

    #[test]
    fn returns_original_xml_when_not_pretty() {
        let original = "<root><child>value</child></root>";
        let formatted = format_xml(original.as_bytes(), false).unwrap();
        assert_eq!(formatted, original.as_bytes());
    }

    #[test]
    fn returns_original_bytes_when_not_pretty() {
        let original = [0xff, 0xfe, 0xfd];
        assert_eq!(format_xml(&original, false).unwrap(), original);
    }

    #[test]
    fn rejects_malformed_xml_when_pretty_printing() {
        let error = format_xml(b"<root></child>", true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to pretty-print XML response"),
            "error: {error:#}"
        );
    }

    #[test]
    fn timeout_minus_one_disables_socket_timeout() {
        assert_eq!(socket_timeout(-1), None);
    }

    #[test]
    fn timeout_zero_is_immediate() {
        assert_eq!(socket_timeout(0), Some(Duration::ZERO));
    }

    #[test]
    fn parses_global_xml_flag_after_transport_subcommand() {
        let cli = Cli::try_parse_from([
            "gvm-cli",
            "socket",
            "--path",
            "/tmp/gvmd.sock",
            "-X",
            "<get_version/>",
        ])
        .expect("cli should parse");

        assert_eq!(cli.xml.as_deref(), Some("<get_version/>"));
        assert!(matches!(cli.transport, Transport::Socket { .. }));
    }
}
