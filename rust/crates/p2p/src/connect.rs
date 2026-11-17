//! Connectivity strategies for peer-lm.
//!
//! `ConnectMode::Auto` tries LAN discovery first, then uses a relay as a
//! rendezvous for DCUtR hole punching (aiming at a direct connection), and
//! keeps the relayed circuit as the fallback when punch fails.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How peer-lm should discover and dial other nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConnectMode {
    /// Respect explicit flags (`dial_via_relay`, `enable_mdns`, …). Default CLI behaviour.
    #[default]
    Manual,
    /// LAN only: mDNS discovery / direct dials. No relay fallback.
    Lan,
    /// Always dial (and preferably listen) via circuit relay. DCUtR may still upgrade.
    Relay,
    /// Progressive: mDNS → relay+DCUtR (prefer direct) → stay on relay if needed.
    Auto,
}

impl ConnectMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Lan => "lan",
            Self::Relay => "relay",
            Self::Auto => "auto",
        }
    }
}

impl std::fmt::Display for ConnectMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ConnectMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "manual" => Ok(Self::Manual),
            "lan" | "mdns" => Ok(Self::Lan),
            "relay" => Ok(Self::Relay),
            "auto" => Ok(Self::Auto),
            other => Err(format!(
                "unknown connect mode '{other}' (expected manual|lan|relay|auto)"
            )),
        }
    }
}

/// Timeouts that drive the `auto` connect ladder.
#[derive(Debug, Clone, Copy)]
pub struct AutoConnectTimers {
    /// How long to wait for mDNS / direct LAN before dialing via relay.
    pub mdns_timeout: Duration,
    /// Extra grace after a relayed dial for DCUtR to finish (logging / metrics).
    pub hole_punch_grace: Duration,
}

impl Default for AutoConnectTimers {
    fn default() -> Self {
        Self {
            mdns_timeout: Duration::from_secs(5),
            hole_punch_grace: Duration::from_secs(15),
        }
    }
}

/// Resolve effective feature flags from a connect mode + user overrides.
#[derive(Debug, Clone)]
pub struct ResolvedConnect {
    pub mode: ConnectMode,
    pub enable_mdns: bool,
    pub enable_dcutr: bool,
    pub reserve_relay: bool,
    /// Initial bootstrap dials use the relay circuit immediately.
    pub dial_via_relay: bool,
    pub timers: AutoConnectTimers,
}

impl ResolvedConnect {
    pub fn resolve(
        mode: ConnectMode,
        enable_mdns: bool,
        enable_dcutr: bool,
        reserve_relay: bool,
        dial_via_relay: bool,
        has_relays: bool,
        timers: AutoConnectTimers,
    ) -> Self {
        match mode {
            ConnectMode::Manual => Self {
                mode,
                enable_mdns,
                enable_dcutr,
                reserve_relay,
                dial_via_relay,
                timers,
            },
            ConnectMode::Lan => Self {
                mode,
                enable_mdns: true,
                enable_dcutr: false,
                reserve_relay: false,
                dial_via_relay: false,
                timers,
            },
            ConnectMode::Relay => Self {
                mode,
                enable_mdns: false,
                enable_dcutr,
                reserve_relay: has_relays,
                dial_via_relay: has_relays,
                timers,
            },
            ConnectMode::Auto => Self {
                mode,
                enable_mdns: true,
                // Hole punch needs a relayled path first; keep DCUtR on when relays exist.
                enable_dcutr: has_relays && enable_dcutr,
                reserve_relay: has_relays,
                // Start on LAN; escalate to relay after mdns_timeout.
                dial_via_relay: false,
                timers,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_forces_mdns_and_defers_relay_dial() {
        let r = ResolvedConnect::resolve(
            ConnectMode::Auto,
            false,
            true,
            false,
            true,
            true,
            AutoConnectTimers::default(),
        );
        assert!(r.enable_mdns);
        assert!(r.enable_dcutr);
        assert!(r.reserve_relay);
        assert!(!r.dial_via_relay);
    }

    #[test]
    fn relay_mode_skips_mdns() {
        let r = ResolvedConnect::resolve(
            ConnectMode::Relay,
            true,
            true,
            false,
            false,
            true,
            AutoConnectTimers::default(),
        );
        assert!(!r.enable_mdns);
        assert!(r.dial_via_relay);
        assert!(r.reserve_relay);
    }
}
