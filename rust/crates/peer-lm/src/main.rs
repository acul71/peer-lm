mod config;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use config::FileConfig;
use libp2p::{identity::Keypair, Multiaddr, PeerId};
use peer_lm_backend::BackendClient;
use peer_lm_gateway::{serve as serve_gateway, GatewayState};
use peer_lm_mock_backend::{router as mock_router, MockState, MOCK_MODEL_ID};
use peer_lm_p2p::{
    run_relay_server, run_rendezvous_server, AutoConnectTimers, CapsInfo, ConnectMode, NodeConfig,
    PeerLmNode,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "peer-lm", about = "OpenAI-compatible local gateway over libp2p")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run gateway + libp2p node (optionally with embedded mock backend)
    Serve(ServeArgs),
    /// Run only the tiny OpenAI-compatible mock backend
    Mock(MockArgs),
    /// Run a Circuit Relay v2 server (for NAT / internet reachability)
    Relay(InfraArgs),
    /// Run a rendezvous point (peer discovery by namespace)
    Rendezvous(InfraArgs),
    /// Print a new peer id / identity info
    Keygen {
        /// Write ed25519 secret protobuf bytes to this path
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ConnectModeArg {
    Manual,
    Lan,
    Relay,
    Auto,
}

impl From<ConnectModeArg> for ConnectMode {
    fn from(v: ConnectModeArg) -> Self {
        match v {
            ConnectModeArg::Manual => ConnectMode::Manual,
            ConnectModeArg::Lan => ConnectMode::Lan,
            ConnectModeArg::Relay => ConnectMode::Relay,
            ConnectModeArg::Auto => ConnectMode::Auto,
        }
    }
}

#[derive(Debug, Parser)]
struct ServeArgs {
    /// YAML config file (see examples/). CLI flags override file values.
    #[arg(long, short = 'c')]
    config: Option<PathBuf>,

    /// Local OpenAI-compatible gateway bind address
    #[arg(long, default_value = "127.0.0.1:3000")]
    gateway: String,

    /// libp2p listen multiaddr
    #[arg(long, default_value = "/ip4/0.0.0.0/tcp/0")]
    listen: String,

    /// Backend base URL (OpenAI-compatible). Ignored if --embedded-mock is set.
    #[arg(long, env = "PEER_LM_BACKEND_URL")]
    backend: Option<String>,

    /// Start an in-process mock backend and use it
    #[arg(long, default_value_t = false)]
    embedded_mock: bool,

    /// Prefer local backend when healthy
    #[arg(long, default_value_t = true)]
    prefer_local: bool,

    /// Do not use a local backend even if configured (force peer routing)
    #[arg(long, default_value_t = false)]
    no_prefer_local: bool,

    /// Connectivity strategy: auto = mDNS → hole-punch/direct → relay
    #[arg(long, value_enum)]
    connect_mode: Option<ConnectModeArg>,

    /// Seconds to wait for mDNS before relay fallback (auto mode)
    #[arg(long)]
    mdns_timeout_secs: Option<u64>,

    /// Extra seconds for DCUtR after a relayed dial (auto mode)
    #[arg(long)]
    hole_punch_grace_secs: Option<u64>,

    /// Enable mDNS LAN discovery
    #[arg(long, default_value_t = true)]
    mdns: bool,

    /// Disable mDNS (forces non-LAN discovery paths)
    #[arg(long, default_value_t = false)]
    no_mdns: bool,

    /// Enable DCUtR hole punching (used with a relay)
    #[arg(long, default_value_t = true)]
    dcutr: bool,

    /// Disable DCUtR
    #[arg(long, default_value_t = false)]
    no_dcutr: bool,

    /// Relay multiaddrs ending in /p2p/<RELAY_ID> (repeatable / comma-separated)
    #[arg(long, value_delimiter = ',')]
    relay: Vec<String>,

    /// Listen for inbound connections via the relay circuit
    #[arg(long, default_value_t = false)]
    reserve_relay: bool,

    /// Dial remote peers through the relay (forces /p2p-circuit path)
    #[arg(long, default_value_t = false)]
    dial_via_relay: bool,

    /// Rendezvous multiaddr ending in /p2p/<ID>
    #[arg(long)]
    rendezvous: Option<String>,

    /// Namespace used on the rendezvous point
    #[arg(long, default_value = "peer-lm")]
    mesh_namespace: String,

    /// Allowlisted peer IDs (comma-separated). Empty = allow all (dev only).
    #[arg(long, default_value = "")]
    allowlist: String,

    /// Peers to dial at startup (comma-separated peer ids); uses relay when --dial-via-relay
    #[arg(long, value_delimiter = ',')]
    dial_peer: Vec<String>,

    /// Path to identity key file (protobuf-encoded ed25519 secret). Created if missing.
    #[arg(long)]
    identity: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct MockArgs {
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: String,

    #[arg(long, default_value = MOCK_MODEL_ID)]
    model: String,
}

#[derive(Debug, Parser)]
struct InfraArgs {
    /// Listen multiaddr (e.g. /ip4/0.0.0.0/tcp/4001)
    #[arg(long, default_value = "/ip4/0.0.0.0/tcp/4001")]
    listen: String,

    /// Identity key path (created if missing)
    #[arg(long)]
    identity: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    match Cli::parse().command {
        Commands::Serve(args) => run_serve(args).await,
        Commands::Mock(args) => run_mock(args).await,
        Commands::Relay(args) => {
            let kp = load_or_create_identity(args.identity.as_deref())?;
            let listen: Multiaddr = args.listen.parse().context("parse listen multiaddr")?;
            run_relay_server(kp, listen).await
        }
        Commands::Rendezvous(args) => {
            let kp = load_or_create_identity(args.identity.as_deref())?;
            let listen: Multiaddr = args.listen.parse().context("parse listen multiaddr")?;
            run_rendezvous_server(kp, listen).await
        }
        Commands::Keygen { out } => run_keygen(out),
    }
}

async fn run_mock(args: MockArgs) -> Result<()> {
    let state = MockState {
        model_id: args.model,
    };
    let app = mock_router(state);
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!("mock backend on http://{}", args.listen);
    axum::serve(listener, app).await?;
    Ok(())
}

fn run_keygen(out: Option<PathBuf>) -> Result<()> {
    let kp = Keypair::generate_ed25519();
    let peer_id = kp.public().to_peer_id();
    println!("{peer_id}");
    if let Some(path) = out {
        let bytes = kp.to_protobuf_encoding()?;
        std::fs::write(&path, bytes)?;
        eprintln!("wrote identity to {}", path.display());
    }
    Ok(())
}

struct ResolvedServe {
    gateway: String,
    listen: String,
    backend: Option<String>,
    embedded_mock: bool,
    prefer_local: bool,
    identity: Option<PathBuf>,
    allowlist: HashSet<PeerId>,
    connect_mode: ConnectMode,
    enable_mdns: bool,
    enable_dcutr: bool,
    reserve_relay: bool,
    dial_via_relay: bool,
    relay_addrs: Vec<Multiaddr>,
    bootstrap_peers: Vec<PeerId>,
    rendezvous_addr: Option<Multiaddr>,
    mesh_namespace: String,
    auto_timers: AutoConnectTimers,
}

fn resolve_serve(args: ServeArgs) -> Result<ResolvedServe> {
    let file = args
        .config
        .as_ref()
        .map(|p| FileConfig::load(p))
        .transpose()?;

    let gateway = file
        .as_ref()
        .and_then(|f| f.gateway.clone())
        .unwrap_or(args.gateway);
    let listen = file
        .as_ref()
        .and_then(|f| f.listen.clone())
        .unwrap_or(args.listen);
    let backend = args
        .backend
        .or_else(|| file.as_ref().and_then(|f| f.backend.clone()));
    let embedded_mock = if args.embedded_mock {
        true
    } else {
        file.as_ref()
            .and_then(|f| f.embedded_mock)
            .unwrap_or(false)
    };
    let prefer_local = if args.no_prefer_local {
        false
    } else if let Some(f) = file.as_ref().and_then(|f| f.prefer_local) {
        f
    } else {
        args.prefer_local
    };
    let identity = args
        .identity
        .or_else(|| file.as_ref().and_then(|f| f.identity.clone()));

    let allowlist = if !args.allowlist.is_empty() {
        parse_allowlist(&args.allowlist)?
    } else if let Some(list) = file.as_ref().and_then(|f| f.allowlist.clone()) {
        parse_peer_ids(&list)?.into_iter().collect()
    } else {
        HashSet::new()
    };

    let connect_mode = if let Some(m) = args.connect_mode {
        ConnectMode::from(m)
    } else if let Some(f) = &file {
        f.connect_mode()?
    } else {
        ConnectMode::Manual
    };

    let enable_mdns = if args.no_mdns {
        false
    } else if let Some(v) = file.as_ref().and_then(|f| f.connect.mdns) {
        v
    } else {
        args.mdns
    };
    let enable_dcutr = if args.no_dcutr {
        false
    } else if let Some(v) = file.as_ref().and_then(|f| f.connect.dcutr) {
        v
    } else {
        args.dcutr
    };

    let mut relay_raw = args.relay;
    if relay_raw.is_empty() {
        if let Some(r) = file.as_ref().and_then(|f| f.connect.relays.clone()) {
            relay_raw = r;
        }
    }
    let relay_addrs = parse_multiaddrs(&relay_raw)?;

    let reserve_relay = if args.reserve_relay {
        true
    } else {
        file.as_ref()
            .and_then(|f| f.connect.reserve_relay)
            .unwrap_or(false)
    };
    let dial_via_relay = if args.dial_via_relay {
        true
    } else {
        file.as_ref()
            .and_then(|f| f.connect.dial_via_relay)
            .unwrap_or(false)
    };

    let mut dial_peers = args.dial_peer;
    if dial_peers.is_empty() {
        if let Some(p) = file.as_ref().and_then(|f| f.connect.dial_peers.clone()) {
            dial_peers = p;
        }
    }
    let bootstrap_peers = parse_peer_ids(&dial_peers)?;

    let rendezvous = args
        .rendezvous
        .or_else(|| file.as_ref().and_then(|f| f.connect.rendezvous.clone()));
    let rendezvous_addr = rendezvous
        .as_deref()
        .map(|s| s.parse::<Multiaddr>().context("parse rendezvous multiaddr"))
        .transpose()?;

    let mesh_namespace = file
        .as_ref()
        .and_then(|f| f.connect.mesh_namespace.clone())
        .unwrap_or(args.mesh_namespace);

    let mut auto_timers = file
        .as_ref()
        .map(|f| f.auto_timers())
        .unwrap_or_default();
    if let Some(s) = args.mdns_timeout_secs {
        auto_timers.mdns_timeout = Duration::from_secs(s);
    }
    if let Some(s) = args.hole_punch_grace_secs {
        auto_timers.hole_punch_grace = Duration::from_secs(s);
    }

    Ok(ResolvedServe {
        gateway,
        listen,
        backend,
        embedded_mock,
        prefer_local,
        identity,
        allowlist,
        connect_mode,
        enable_mdns,
        enable_dcutr,
        reserve_relay,
        dial_via_relay,
        relay_addrs,
        bootstrap_peers,
        rendezvous_addr,
        mesh_namespace,
        auto_timers,
    })
}

async fn run_serve(args: ServeArgs) -> Result<()> {
    let cfg = resolve_serve(args)?;
    let keypair = load_or_create_identity(cfg.identity.as_deref())?;
    let local_peer_id = keypair.public().to_peer_id();
    tracing::info!(%local_peer_id, "local peer id");

    let mut backend_url = cfg.backend.clone();
    if cfg.embedded_mock {
        let mock_listen = "127.0.0.1:0";
        let listener = tokio::net::TcpListener::bind(mock_listen).await?;
        let addr = listener.local_addr()?;
        let mock_url = format!("http://{addr}");
        let state = MockState::default();
        tokio::spawn(async move {
            let app = mock_router(state);
            if let Err(err) = axum::serve(listener, app).await {
                tracing::error!(error = %err, "embedded mock exited");
            }
        });
        tracing::info!(%mock_url, "embedded mock backend started");
        backend_url = Some(mock_url);
    }

    let backend = match backend_url {
        Some(url) => Some(BackendClient::new(url)?),
        None => None,
    };

    let models = if let Some(ref b) = backend {
        match b.get_models_json().await {
            Ok(v) => v
                .get("data")
                .and_then(|d| d.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_else(|| vec![MOCK_MODEL_ID.to_string()]),
            Err(_) => vec![MOCK_MODEL_ID.to_string()],
        }
    } else {
        vec![]
    };

    let local_caps = CapsInfo {
        models,
        engine: if cfg.embedded_mock {
            "mock".into()
        } else {
            "openai-compat".into()
        },
        slots_free: if backend.is_some() { 1 } else { 0 },
        max_ctx: 8192,
        busy: false,
    };

    let listen: Multiaddr = cfg.listen.parse().context("parse listen multiaddr")?;
    let (node, _swarm_task) = PeerLmNode::start(
        NodeConfig {
            listen,
            enable_mdns: cfg.enable_mdns,
            enable_dcutr: cfg.enable_dcutr,
            allowlist: cfg.allowlist,
            local_caps,
            keypair,
            relay_addrs: cfg.relay_addrs,
            reserve_relay: cfg.reserve_relay,
            dial_via_relay: cfg.dial_via_relay,
            rendezvous_addr: cfg.rendezvous_addr,
            mesh_namespace: cfg.mesh_namespace,
            bootstrap_peers: cfg.bootstrap_peers,
            connect_mode: cfg.connect_mode,
            auto_timers: cfg.auto_timers,
        },
        backend.clone(),
    )
    .await?;

    let state = GatewayState {
        backend,
        node,
        prefer_local: cfg.prefer_local,
    };
    serve_gateway(&cfg.gateway, state).await?;
    Ok(())
}

fn load_or_create_identity(path: Option<&std::path::Path>) -> Result<Keypair> {
    let Some(path) = path else {
        return Ok(Keypair::generate_ed25519());
    };
    if path.exists() {
        let bytes = std::fs::read(path)?;
        Ok(Keypair::from_protobuf_encoding(&bytes)?)
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let kp = Keypair::generate_ed25519();
        std::fs::write(path, kp.to_protobuf_encoding()?)?;
        tracing::info!(path = %path.display(), "created new identity");
        Ok(kp)
    }
}

fn parse_allowlist(raw: &str) -> Result<HashSet<PeerId>> {
    Ok(parse_peer_ids(
        &raw.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>(),
    )?
    .into_iter()
    .collect())
}

fn parse_peer_ids(raw: &[String]) -> Result<Vec<PeerId>> {
    raw.iter()
        .map(|s| PeerId::from_str(s).with_context(|| format!("invalid peer id {s}")))
        .collect()
}

fn parse_multiaddrs(raw: &[String]) -> Result<Vec<Multiaddr>> {
    raw.iter()
        .map(|s| s.parse::<Multiaddr>().with_context(|| format!("invalid multiaddr {s}")))
        .collect()
}
