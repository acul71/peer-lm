use axum::Router;
use peer_lm_backend::BackendClient;
use peer_lm_mock_backend::{router, MockState};
use serde_json::json;

async fn spawn_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app: Router = router(MockState::default());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn health_and_models() {
    let (url, _h) = spawn_mock().await;
    let client = BackendClient::new(&url).unwrap();
    let health = client.health().await;
    assert!(health.ok, "{health:?}");
    let models = client.get_models_json().await.unwrap();
    assert_eq!(models["data"][0]["id"], "peer-lm-mock");
}

#[tokio::test]
async fn forward_chat_completions() {
    let (url, _h) = spawn_mock().await;
    let client = BackendClient::new(&url).unwrap();
    let body = serde_json::to_vec(&json!({
        "model": "peer-lm-mock",
        "messages": [{"role":"user","content":"backend client"}]
    }))
    .unwrap();
    let resp = client
        .forward_chat_completions(body.into(), false)
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    let v: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    let content = v["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(content.contains("backend client"));
}

#[tokio::test]
async fn forward_streaming_collects_sse() {
    let (url, _h) = spawn_mock().await;
    let client = BackendClient::new(&url).unwrap();
    let body = serde_json::to_vec(&json!({
        "model": "peer-lm-mock",
        "stream": true,
        "messages": [{"role":"user","content":"sse"}]
    }))
    .unwrap();
    let resp = client
        .forward_chat_completions(body.into(), true)
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    let text = String::from_utf8(resp.body.to_vec()).unwrap();
    assert!(text.contains("data:"));
    assert!(text.contains("[DONE]"));
}
