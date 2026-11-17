//! Integration: relay + two nodes + mock backend; infer over /p2p-circuit.
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use peer_lm_backend::BackendClient;
use peer_lm_mock_backend::{router as mock_router, MockState, MOCK_MODEL_ID};
use peer_lm_p2p::{
    run_relay_server, CapsInfo, InferResponse, NodeConfig, PeerLmNode,
};
use serde_json::json;
use std::collections::HashSet;
use std::time::Duration;

async fn spawn_mock() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = mock_router(MockState::default());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn spawn_relay() -> (Multiaddr, Keypair) {
    let keypair = Keypair::generate_ed25519();
    let peer_id = keypair.public().to_peer_id();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let listen: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}").parse().unwrap();
    let addr = listen
        .clone()
        .with(libp2p::multiaddr::Protocol::P2p(peer_id));
    let kp = keypair.clone();
    tokio::spawn(async move {
        let _ = run_relay_server(kp, listen).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    (addr, keypair)
}

async fn start_node(
    keypair: Keypair,
    backend: Option<BackendClient>,
    relay: Multiaddr,
    reserve: bool,
    dial_via_relay: bool,
    bootstrap: Vec<libp2p::PeerId>,
    allowlist: HashSet<libp2p::PeerId>,
) -> PeerLmNode {
    let models = if backend.is_some() {
        vec![MOCK_MODEL_ID.to_string()]
    } else {
        vec![]
    };
    let (node, _task) = PeerLmNode::start(
        NodeConfig {
            listen: "/ip4/127.0.0.1/tcp/0".parse().unwrap(),
            enable_mdns: false,
            enable_dcutr: true,
            allowlist,
            local_caps: CapsInfo {
                models,
                engine: "mock".into(),
                slots_free: if backend.is_some() { 1 } else { 0 },
                max_ctx: 8192,
                busy: false,
            },
            keypair,
            relay_addrs: vec![relay],
            reserve_relay: reserve,
            dial_via_relay,
            rendezvous_addr: None,
            mesh_namespace: "peer-lm-test".into(),
            bootstrap_peers: bootstrap,
            connect_mode: peer_lm_p2p::ConnectMode::Manual,
            auto_timers: peer_lm_p2p::AutoConnectTimers::default(),
        },
        backend,
    )
    .await
    .unwrap();
    node
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn infer_through_local_relay() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_test_writer()
        .try_init();

    let mock_url = spawn_mock().await;
    let (relay_addr, _) = spawn_relay().await;

    let key_a = Keypair::generate_ed25519();
    let key_b = Keypair::generate_ed25519();
    let peer_a = key_a.public().to_peer_id();
    let peer_b = key_b.public().to_peer_id();

    let backend = BackendClient::new(&mock_url).unwrap();
    let node_a = start_node(
        key_a,
        Some(backend),
        relay_addr.clone(),
        true,
        false,
        vec![],
        HashSet::from([peer_b]),
    )
    .await;

    // Give A time to Identify + reserve on the relay.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let node_b = start_node(
        key_b,
        None,
        relay_addr,
        false,
        true,
        vec![peer_a],
        HashSet::from([peer_a]),
    )
    .await;

    let body = serde_json::to_vec(&json!({
        "model": MOCK_MODEL_ID,
        "messages": [{"role":"user","content":"relay integration"}]
    }))
    .unwrap();

    let mut last_err = None;
    let mut response: Option<InferResponse> = None;
    for _ in 0..40 {
        match node_b.infer(peer_a, body.clone(), false).await {
            Ok(resp) => {
                response = Some(resp);
                break;
            }
            Err(err) => {
                last_err = Some(err.to_string());
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }

    let resp = response.unwrap_or_else(|| {
        panic!(
            "infer failed after retries; last_err={last_err:?}; a={}; b={}",
            node_a.local_peer_id(),
            node_b.local_peer_id()
        )
    });
    assert_eq!(resp.status, 200);
    let v: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    let content = v["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(
        content.contains("relay integration"),
        "unexpected content: {content}"
    );
}
