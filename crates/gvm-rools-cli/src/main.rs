// SPDX-License-Identifier: AGPL-3.0-or-later

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use gvm_connection::{GvmConnection, SshAuth, SshConfig, SshConnection, UnixSocketConfig, UnixSocketConnection};
use gvm_protocol::Response;

#[derive(Parser, Debug)]
#[command(name = "gvm-cli")]
#[command(about = "Rust reimplementation of gvm-tools gvm-cli (GMP only)", long_about = None)]
struct Cli {
    /// GMP username (optional; if provided, gvm-cli will authenticate before sending the command)
    #[arg(long, env = "GMP_USERNAME")]
    gmp_username: Option<String>,

    /// GMP password (optional; if omitted but username is provided, gvm-cli will prompt)
    #[arg(long, env = "GMP_PASSWORD")]
    gmp_password: Option<String>,

    /// XML request to send (if omitted, read from infile or stdin)
    #[arg(short = 'X', long)]
    xml: Option<String>,

    /// Return raw XML even for non-2xx responses (do not treat as error)
    #[arg(short = 'r', long, default_value_t = false)]
    raw: bool,

    /// Pretty format the returned XML (MVP: not implemented; currently prints as-is)
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
        #[arg(long)]
        password: Option<String>,

        /// Remote gvmd socket path
        #[arg(long, default_value = "/run/gvmd/gvmd.sock")]
        remote_socket: String,
    },

    /// TLS transport (not yet implemented in rust-gvm)
    Tls {},
}

fn read_xml(cli: &Cli) -> Result<String> {
    if let Some(xml) = &cli.xml {
        return Ok(xml.clone());
    }

    if let Some(path) = &cli.infile {
        return std::fs::read_to_string(path)
            .with_context(|| format!("failed to read infile {}", path.display()));
    }

    // stdin
    let mut buf = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
        .context("failed to read stdin")?;
    Ok(buf)
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

    let password = password.ok_or_else(|| anyhow!("--gmp-password is required when --gmp-username is set (prompting not implemented yet)"))?;

    let auth_xml = format!(
        "<authenticate><credentials><username>{}</username><password>{}</password></credentials></authenticate>",
        username, password
    );

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
        let text = resp.status_text().unwrap_or_else(|| "<no status text>".to_string());
        return Err(anyhow!("authentication failed (status {status}): {text}"));
    }

    Ok(())
}

async fn run(cli: Cli) -> Result<i32> {
    if cli.pretty {
        // MVP: keep behavior but make it explicit.
        eprintln!("warning: --pretty is not implemented yet; printing XML as-is");
    }

    let xml = read_xml(&cli)?.trim().to_string();
    if xml.is_empty() {
        return Err(anyhow!("no XML provided (use --xml, infile, or stdin)"));
    }

    let mut conn: Box<dyn GvmConnection> = match cli.transport {
        Transport::Socket { path, timeout } => {
            let timeout = if timeout < 0 { None } else { Some(std::time::Duration::from_secs(timeout as u64)) };
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
                .map(SshAuth::Password)
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
        cli.gmp_password.as_deref(),
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
        let text = resp.status_text().unwrap_or_else(|| "<no status text>".to_string());
        eprintln!("server rejected command (status {status}): {text}");
        // Still print the response to stdout for debugging.
        print!("{}", String::from_utf8_lossy(resp.as_ref()));
        return Ok(1);
    }

    print!("{}", String::from_utf8_lossy(resp.as_ref()));

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
