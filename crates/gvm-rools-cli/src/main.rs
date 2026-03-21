#![deny(unsafe_code)]

// SPDX-License-Identifier: AGPL-3.0-or-later

use std::io::{IsTerminal, Write};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use gvm_connection::{
    GvmConnection, SshAuth, SshConfig, SshConnection, UnixSocketConfig, UnixSocketConnection,
};
use gvm_protocol::Response;
use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg};
use quick_xml::escape::escape;
use quick_xml::events::Event;
use quick_xml::{Reader, Writer};
use zeroize::Zeroizing;

#[derive(Parser, Debug)]
#[command(name = "gvm-cli")]
#[command(about = "Rust reimplementation of gvm-tools gvm-cli (GMP only)", long_about = None)]
struct Cli {
    /// GMP username (optional; if provided, gvm-cli will authenticate before sending the command)
    #[arg(long, env = "GMP_USERNAME")]
    gmp_username: Option<String>,

    /// GMP password (optional; if omitted but username is provided, gvm-cli will prompt)
    #[arg(long, env = "GMP_PASSWORD", value_parser = parse_secret)]
    gmp_password: Option<Zeroizing<String>>,

    /// XML request to send (if omitted, read from infile or stdin)
    #[arg(short = 'X', long)]
    xml: Option<String>,

    /// Return raw XML even for non-2xx responses (do not treat as error)
    #[arg(short = 'r', long, default_value_t = false)]
    raw: bool,

    /// Pretty format the returned XML
    #[arg(long, default_value_t = false)]
    pretty: bool,

    /// Measure command execution time
    #[arg(long, default_value_t = false)]
    duration: bool,

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

        /// Password authentication (if omitted, SSH agent will be used)
        #[arg(long, value_parser = parse_secret)]
        password: Option<Zeroizing<String>>,

        /// Remote gvmd socket path
        #[arg(long, default_value = "/run/gvmd/gvmd.sock")]
        remote_socket: String,
    },

    /// TLS transport (not yet implemented in rust-gvm)
    Tls {},
}

fn parse_secret(value: &str) -> Result<Zeroizing<String>, String> {
    Ok(Zeroizing::new(value.to_owned()))
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
    tokio::task::spawn_blocking(|| {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .context("failed to read stdin")?;
        Ok::<_, anyhow::Error>(buf)
    })
    .await
    .context("stdin read task failed")?
}

async fn authenticate_if_needed<C: GvmConnection + ?Sized>(
    conn: &mut C,
    username: Option<&str>,
    password: Option<&str>,
    raw: bool,
) -> Result<()> {
    let Some(username) = username else {
        return Ok(());
    };

    let password =
        password.ok_or_else(|| anyhow!("--gmp-password is required when --gmp-username is set"))?;

    let auth_xml = build_auth_xml(username, password);

    conn.send(auth_xml.as_bytes())
        .await
        .context("send authenticate failed")?;
    let resp_bytes = conn.read().await.context("read authenticate failed")?;
    let resp = Response::new(resp_bytes);

    if raw {
        return Ok(());
    }

    if !resp.is_success() {
        let status = resp.status_code().unwrap_or(0);
        let text = resp
            .status_text()
            .unwrap_or_else(|| "<no status text>".to_string());
        return Err(anyhow!("authentication failed (status {status}): {text}"));
    }

    Ok(())
}

fn build_auth_xml(username: &str, password: &str) -> String {
    let username = escape(username);
    let password = escape(password);
    format!(
        "<authenticate><credentials><username>{username}</username><password>{password}</password></credentials></authenticate>"
    )
}

async fn resolve_gmp_password(cli: &mut Cli) -> Result<Option<Zeroizing<String>>> {
    let Some(_) = cli.gmp_username.as_deref() else {
        return Ok(cli.gmp_password.take());
    };

    if let Some(password) = cli.gmp_password.take() {
        return Ok(Some(password));
    }

    if std::io::stdin().is_terminal() {
        return tokio::task::spawn_blocking(|| prompt_password_from_tty("GMP Password: "))
            .await
            .context("GMP password prompt task failed")?
            .map(Zeroizing::new)
            .map(Some)
            .context("failed to read GMP password from TTY");
    }

    Err(anyhow!(
        "--gmp-password is required when --gmp-username is set"
    ))
}

fn prompt_password_from_tty(prompt: &str) -> std::io::Result<String> {
    let stdin = std::io::stdin();
    let mut stderr = std::io::stderr().lock();

    stderr.write_all(prompt.as_bytes())?;
    stderr.flush()?;

    let mut termios = tcgetattr(stdin.as_fd())?;
    let original = termios.clone();
    termios.local_flags.remove(LocalFlags::ECHO);
    tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &termios)?;

    let mut password = String::new();
    let read_result = stdin.read_line(&mut password);

    let restore_result = tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &original);
    stderr.write_all(b"\n")?;
    stderr.flush()?;

    restore_result?;

    read_result?;
    while matches!(password.chars().last(), Some('\n' | '\r')) {
        password.pop();
    }
    Ok(password)
}

fn format_xml(xml: &[u8], pretty: bool) -> Result<String> {
    if !pretty {
        return Ok(String::from_utf8_lossy(xml).into_owned());
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

    String::from_utf8(writer.into_inner()).context("pretty-printed XML was not valid UTF-8")
}

async fn run(mut cli: Cli) -> Result<i32> {
    let xml = read_xml(&cli).await?.trim().to_string();
    if xml.is_empty() {
        return Err(anyhow!("no XML provided (use --xml, infile, or stdin)"));
    }
    let gmp_password = resolve_gmp_password(&mut cli).await?;

    let mut conn: Box<dyn GvmConnection> = match cli.transport {
        Transport::Socket { path, timeout } => {
            let timeout = if timeout < 0 {
                None
            } else {
                Some(std::time::Duration::from_secs(timeout as u64))
            };
            let mut cfg = UnixSocketConfig::new(path);
            if let Some(t) = timeout {
                cfg = cfg.with_timeout(t);
            }
            Box::new(UnixSocketConnection::new(cfg))
        }
        Transport::Ssh {
            hostname,
            port,
            username,
            password,
            remote_socket,
        } => {
            let auth = password
                .map(|password| SshAuth::Password(password.to_string()))
                .unwrap_or(SshAuth::Agent);
            let cfg = SshConfig::new(hostname, username, auth)
                .with_port(port)
                .with_remote_socket(remote_socket);
            Box::new(SshConnection::new(cfg))
        }
        Transport::Tls {} => {
            return Err(anyhow!(
                "TLS transport not implemented yet (see rust-gvm TLS transport issue)"
            ));
        }
    };

    conn.connect().await.context("connect failed")?;

    authenticate_if_needed(
        conn.as_mut(),
        cli.gmp_username.as_deref(),
        gmp_password.as_deref().map(String::as_str),
        cli.raw,
    )
    .await?;

    let start = Instant::now();
    conn.send(xml.as_bytes()).await.context("send failed")?;
    let resp_bytes = conn.read().await.context("read failed")?;
    let elapsed = start.elapsed();

    let resp = Response::new(resp_bytes);

    if !cli.raw && !resp.is_success() {
        let status = resp.status_code().unwrap_or(0);
        let text = resp
            .status_text()
            .unwrap_or_else(|| "<no status text>".to_string());
        eprintln!("server rejected command (status {status}): {text}");
        // Still print the response to stdout for debugging.
        print!("{}", format_xml(resp.as_ref(), cli.pretty)?);
        return Ok(1);
    }

    print!("{}", format_xml(resp.as_ref(), cli.pretty)?);

    if cli.duration {
        eprintln!("Elapsed time: {:.3} seconds", elapsed.as_secs_f64());
    }

    conn.disconnect().await.ok();

    Ok(0)
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
    use super::{build_auth_xml, format_xml};

    #[test]
    fn pretty_prints_xml_with_indentation() {
        let formatted = format_xml(b"<root><child>value</child></root>", true).unwrap();
        assert!(formatted.contains("\n  <child>value</child>\n"));
    }

    #[test]
    fn returns_original_xml_when_not_pretty() {
        let original = "<root><child>value</child></root>";
        let formatted = format_xml(original.as_bytes(), false).unwrap();
        assert_eq!(formatted, original);
    }

    #[test]
    fn test_xml_escape_in_credentials() {
        let xml = build_auth_xml(r#"<user>&""#, r#"<pass>&">"#);
        assert!(xml.contains("<username>&lt;user&gt;&amp;&quot;</username>"));
        assert!(xml.contains("<password>&lt;pass&gt;&amp;&quot;&gt;</password>"));
    }
}
