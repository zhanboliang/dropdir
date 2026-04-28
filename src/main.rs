mod fs_ops;
mod routes;
mod text_ext;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context};
use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post},
    Router,
};

use crate::routes::{
    auth_middleware, delete_entry, download, index, list, read_file, rename, upload, write_file,
    AppState,
};

const DEFAULT_PORT: u16 = 8089;
const UPLOAD_LIMIT: usize = 1024 * 1024 * 1024; // 1 GiB

struct Args {
    root: Option<PathBuf>,
    host: String,
    port: u16,
    token: Option<String>,
    no_auth: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            root: None,
            host: "127.0.0.1".to_string(),
            port: DEFAULT_PORT,
            token: None,
            no_auth: false,
        }
    }
}

fn print_help() {
    println!(
        "dropdir - a tiny local file manager served from a directory.

USAGE:
    dropdir [DIR] [OPTIONS]

ARGS:
    <DIR>               Directory to serve. Must be the first argument if
                        provided. Defaults to the current working directory.

OPTIONS:
    --host <HOST>       Bind address (default: 127.0.0.1)
    --open              Shortcut for --host 0.0.0.0 (expose to LAN)
    --port <PORT>       Port (default: {DEFAULT_PORT})
    --token <TOKEN>     Use the given auth token instead of a random one
    --no-auth           Disable auth. DANGEROUS on shared networks.
    -h, --help          Show this help and exit

EXAMPLES:
    dropdir                              # serve the current directory
    dropdir /Users/me/Downloads          # serve a specific directory
    dropdir ./project --open --port 9000 # LAN share of ./project on :9000

SECURITY:
    * Defaults to 127.0.0.1 only; use --open to expose on the LAN.
    * A random 128-bit auth token is generated at startup unless --no-auth
      or --token is passed. Open the printed URL in your browser.
    * Upload/edit of native-executable or shell-script filenames is refused.
    * Symlinks are not written through; path traversal is blocked."
    );
}

fn parse_args() -> anyhow::Result<Args> {
    let mut a = Args::default();
    let raw: Vec<String> = std::env::args().skip(1).collect();

    // First positional argument (must be position 1, i.e. immediately after
    // the program name) is the directory to serve. A leading `-` means a
    // flag instead, and we fall back to the current working directory.
    let mut i = 0;
    if let Some(first) = raw.first() {
        if !first.starts_with('-') {
            a.root = Some(PathBuf::from(first));
            i = 1;
        }
    }

    while i < raw.len() {
        let arg = raw[i].as_str();
        match arg {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "--host" => {
                i += 1;
                a.host = raw
                    .get(i)
                    .cloned()
                    .ok_or_else(|| anyhow!("--host needs a value"))?;
            }
            "--port" => {
                i += 1;
                let v = raw
                    .get(i)
                    .ok_or_else(|| anyhow!("--port needs a value"))?;
                a.port = v.parse().with_context(|| format!("invalid port: {v}"))?;
            }
            "--token" => {
                i += 1;
                a.token = Some(
                    raw.get(i)
                        .cloned()
                        .ok_or_else(|| anyhow!("--token needs a value"))?,
                );
            }
            "--no-auth" => {
                a.no_auth = true;
            }
            "--open" => {
                a.host = "0.0.0.0".to_string();
            }
            other => {
                return Err(anyhow!("unknown argument: {other}\n(run with --help)"));
            }
        }
        i += 1;
    }
    Ok(a)
}

fn random_token() -> anyhow::Result<String> {
    let mut buf = [0u8; 16]; // 128 bits
    getrandom::getrandom(&mut buf).map_err(|e| anyhow!("getrandom failed: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Enumerate all non-loopback IPv4 addresses, paired with the interface name.
/// When a machine has VPN / virtual adapters the "outbound route" heuristic
/// picks the wrong one, so we just list every candidate the way `npm run dev`
/// does and let the user pick.
fn list_lan_addrs() -> Vec<(String, std::net::IpAddr)> {
    let mut out = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            let ip = iface.ip();
            if iface.is_loopback() || !ip.is_ipv4() {
                continue;
            }
            out.push((iface.name, ip));
        }
    }
    out
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };

    let requested_root = match args.root.as_ref() {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };
    let cwd = requested_root.canonicalize().with_context(|| {
        format!(
            "cannot resolve serve directory: {}",
            requested_root.display()
        )
    })?;
    if !cwd.is_dir() {
        bail!("not a directory: {}", cwd.display());
    }

    let token = if args.no_auth {
        String::new()
    } else if let Some(t) = args.token {
        if t.is_empty() {
            return Err(anyhow!("--token cannot be empty (use --no-auth instead)"));
        }
        t
    } else {
        random_token()?
    };

    let state = Arc::new(AppState {
        root: cwd.clone(),
        token: token.clone(),
    });

    let mut app = Router::new()
        .route("/", get(index))
        .route("/api/list", get(list))
        .route("/api/read", get(read_file))
        .route("/api/write", post(write_file))
        .route("/api/upload", post(upload))
        .route("/api/rename", post(rename))
        .route("/api/delete", delete(delete_entry))
        .route("/api/download", get(download));

    app = app.layer(middleware::from_fn_with_state(
        state.clone(),
        auth_middleware,
    ));

    let app = app
        .layer(DefaultBodyLimit::max(UPLOAD_LIMIT))
        .with_state(state);

    let addr = format!("{}:{}", args.host, args.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {addr}"))?;

    let token_suffix = if token.is_empty() {
        String::new()
    } else {
        format!("?t={token}")
    };

    println!("dropdir");
    println!("  serving    : {}", cwd.display());
    println!("  bind       : {addr}");
    if args.host == "0.0.0.0" {
        println!("  ⚠ open to LAN — anyone on this network can reach the URL below");
    }
    if token.is_empty() {
        println!("  auth       : DISABLED (--no-auth)");
    } else {
        println!("  auth token : {token}");
    }
    if args.host == "0.0.0.0" {
        println!(
            "  local      : http://127.0.0.1:{port}/{suffix}",
            port = args.port,
            suffix = token_suffix
        );
        let lan = list_lan_addrs();
        if lan.is_empty() {
            println!("  network    : (no LAN interface detected)");
        } else {
            for (i, (name, ip)) in lan.iter().enumerate() {
                let label = if i == 0 { "network" } else { "       " };
                println!(
                    "  {label}    : http://{ip}:{port}/{suffix}  ({name})",
                    port = args.port,
                    suffix = token_suffix
                );
            }
        }
    } else {
        println!(
            "  open url   : http://{host}:{port}/{suffix}",
            host = args.host,
            port = args.port,
            suffix = token_suffix
        );
    }
    println!("  upload cap : {} MiB", UPLOAD_LIMIT / 1024 / 1024);
    println!("press Ctrl+C to stop.");
    use std::io::Write;
    let _ = std::io::stdout().flush();

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("\nshutting down...");
}
