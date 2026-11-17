use libp2p::identity::Keypair;
use peer_lm_backend::BackendClient;
use peer_lm_gateway::{router, GatewayState};
use peer_lm_mock_backend::{router as mock_router, MockState, MOCK_MODEL_ID};
use peer_lm_p2p::{CapsInfo, NodeConfig, PeerLmNode};
use reqwest::Client;
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

async fn spawn_gateway(backend_url: &str) -> (String, PeerLmNode) {
    let backend = BackendClient::new(backend_url).unwrap();
    let keypair = Keypair::generate_ed25519();
    let (node, _task) = PeerLmNode::start(
        NodeConfig {
            listen: "/ip4/127.0.0.1/tcp/0".parse().unwrap(),
            enable_mdns: false,
            enable_dcutr: false,
            allowlist: HashSet::new(),
            local_caps: CapsInfo {
                models: vec![MOCK_MODEL_ID.to_string()],
                engine: "mock".into(),
                slots_free: 1,
                max_ctx: 8192,
                busy: false,
            },
            keypair,
            relay_addrs: vec![],
            reserve_relay: false,
            dial_via_relay: false,
            rendezvous_addr: None,
            mesh_namespace: "peer-lm-test".into(),
            bootstrap_peers: vec![],
            connect_mode: peer_lm_p2p::ConnectMode::Manual,
            auto_timers: peer_lm_p2p::AutoConnectTimers::default(),
        },
        Some(backend.clone()),
    )
    .await
    .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = GatewayState {
        backend: Some(backend),
        node: node.clone(),
        prefer_local: true,
    };
    let app = router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), node)
}

#[tokio::test]
async fn gateway_proxies_local_mock() {
    let mock_url = spawn_mock().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (gw, node) = spawn_gateway(&mock_url).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::new();
    let health: serde_json::Value = client
        .get(format!("{gw}/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["status"], "ok");
    assert_eq!(health["local_backend_ok"], true);
    assert_eq!(
        health["local_peer_id"].as_str().unwrap(),
        node.local_peer_id().to_string()
    );

    let chat: serde_json::Value = client
        .post(format!("{gw}/v1/chat/completions"))
        .json(&serde_json::json!({
            "model": MOCK_MODEL_ID,
            "messages": [{"role":"user","content":"gateway local"}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let content = chat["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(content.contains("gateway local"));
}
