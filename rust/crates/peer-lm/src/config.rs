//! YAML configuration for `peer-lm serve`.
//!
//! Load with `--config path.yaml`. Explicit CLI flags override file values.

use anyhow::{Context, Result};
use peer_lm_p2p::{AutoConnectTimers, ConnectMode};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct FileConfig {
    /// OpenAI-compatible gateway bind (`host:port`).
    pub gateway: Option<String>,
    /// libp2p listen multiaddr.
    pub listen: Option<String>,
    /// OpenAI-compatible backend base URL.
    pub backend: Option<String>,
    /// Start the in-process mock backend.
    pub embedded_mock: Option<bool>,
    /// Prefer a healthy local backend before peers.
    pub prefer_local: Option<bool>,
    /// Identity key path (created if missing).
    pub identity: Option<PathBuf>,
    /// Allowlisted peer IDs. Empty / omitted = allow all (dev only).
    pub allowlist: Option<Vec<String>>,
    pub connect: ConnectFileConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ConnectFileConfig {
    /// `auto` | `lan` | `relay` | `manual`
    pub mode: Option<String>,
    pub mdns: Option<bool>,
    pub dcutr: Option<bool>,
    pub reserve_relay: Option<bool>,
    pub dial_via_relay: Option<bool>,
    /// Seconds to wait for mDNS before relay fallback (`auto` mode).
    pub mdns_timeout_secs: Option<u64>,
    /// Extra seconds allowed for DCUtR after a relayed dial (`auto` mode).
    pub hole_punch_grace_secs: Option<u64>,
    /// Relay multiaddrs ending in `/p2p/<RELAY_ID>`.
    pub relays: Option<Vec<String>>,
    /// Peer IDs to dial at startup.
    pub dial_peers: Option<Vec<String>>,
    /// Optional rendezvous multiaddr ending in `/p2p/<ID>`.
    pub rendezvous: Option<String>,
    pub mesh_namespace: Option<String>,
}

impl FileConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let cfg: Self = serde_yaml::from_str(&raw)
            .with_context(|| format!("parse config YAML {}", path.display()))?;
        Ok(cfg)
    }

    pub fn connect_mode(&self) -> Result<ConnectMode> {
        match self.connect.mode.as_deref() {
            None => Ok(ConnectMode::Manual),
            Some(s) => s.parse().map_err(anyhow::Error::msg),
        }
    }

    pub fn auto_timers(&self) -> AutoConnectTimers {
        let mut t = AutoConnectTimers::default();
        if let Some(s) = self.connect.mdns_timeout_secs {
            t.mdns_timeout = Duration::from_secs(s);
        }
        if let Some(s) = self.connect.hole_punch_grace_secs {
            t.hole_punch_grace = Duration::from_secs(s);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auto_example() {
        let yaml = r#"
gateway: "127.0.0.1:3000"
backend: "http://127.0.0.1:8080"
prefer_local: false
identity: "./data/client.key"
allowlist:
  - "12D3KooWExamplePeer"
connect:
  mode: auto
  mdns_timeout_secs: 3
  hole_punch_grace_secs: 20
  relays:
    - "/ip4/127.0.0.1/tcp/4444/p2p/12D3KooWRelay"
  dial_peers:
    - "12D3KooWExamplePeer"
"#;
        let cfg: FileConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.connect_mode().unwrap(), ConnectMode::Auto);
        assert_eq!(cfg.auto_timers().mdns_timeout, Duration::from_secs(3));
        assert_eq!(cfg.connect.relays.unwrap().len(), 1);
    }
}
