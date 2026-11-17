use anyhow::Result;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use peer_lm_backend::BackendClient;
use peer_lm_p2p::PeerLmNode;
use serde_json::{json, Value};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct GatewayState {
    pub backend: Option<BackendClient>,
    pub node: PeerLmNode,
    pub prefer_local: bool,
}

pub fn router(state: GatewayState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/peer-lm/peers", get(peers))
        .layer(TraceLayer::new_for_http())
        .with_state(Arc::new(state))
}

async fn health(State(state): State<Arc<GatewayState>>) -> Json<Value> {
    let local = match &state.backend {
        Some(b) => b.health().await,
        None => peer_lm_backend::BackendHealth {
            ok: false,
            detail: "no local backend".into(),
        },
    };
    let peer_count = state.node.list_peers().await.map(|p| p.len()).unwrap_or(0);
    Json(json!({
        "status": "ok",
        "local_peer_id": state.node.local_peer_id().to_string(),
        "local_backend_ok": local.ok,
        "local_backend_detail": local.detail,
        "peers": peer_count,
    }))
}

async fn peers(State(state): State<Arc<GatewayState>>) -> Json<Value> {
    let peers = state.node.list_peers().await.unwrap_or_default();
    let data: Vec<Value> = peers
        .into_iter()
        .map(|p| {
            json!({
                "peer_id": p.peer_id.to_string(),
                "addresses": p.addresses.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                "caps": p.caps,
            })
        })
        .collect();
    Json(json!({ "peers": data }))
}

async fn models(State(state): State<Arc<GatewayState>>) -> Response {
    if let Some(backend) = &state.backend {
        if backend.health().await.ok {
            if let Ok(v) = backend.get_models_json().await {
                return Json(v).into_response();
            }
        }
    }

    // Aggregate from peers
    let peers = state.node.list_peers().await.unwrap_or_default();
    let mut models = Vec::new();
    for p in peers {
        if let Some(caps) = p.caps {
            for m in caps.models {
                models.push(json!({
                    "id": m,
                    "object": "model",
                    "created": 0,
                    "owned_by": format!("peer:{}", p.peer_id),
                }));
            }
        }
    }
    Json(json!({ "object": "list", "data": models })).into_response()
}

async fn chat_completions(State(state): State<Arc<GatewayState>>, body: Bytes) -> Response {
    let stream = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false);
    let model = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("model")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        });

    // 1) Local backend if healthy
    if state.prefer_local {
        if let Some(backend) = &state.backend {
            if backend.health().await.ok {
                match backend
                    .forward_chat_completions(body.clone(), stream)
                    .await
                {
                    Ok(fwd) => return forwarded_to_response(fwd.status, &fwd.headers, fwd.body),
                    Err(err) => {
                        tracing::warn!(error = %err, "local backend failed; trying peers");
                    }
                }
            }
        }
    }

    // 2) Remote peer (prefer advertised model; else any known/bootstrap peer)
    let peer = match state.node.pick_peer_for_model(model.as_deref()).await {
        Some(p) => p,
        None => {
            let peers = state.node.list_peers().await.unwrap_or_default();
            match peers.into_iter().next().map(|p| p.peer_id) {
                Some(p) => p,
                None => {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({
                            "error": {
                                "message": "no local backend and no capable peers",
                                "type": "peer_lm_unavailable"
                            }
                        })),
                    )
                        .into_response();
                }
            }
        }
    };

    match state.node.infer(peer, body.to_vec(), stream).await {
        Ok(resp) => {
            let mut response = Response::builder()
                .status(StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK))
                .body(Body::from(resp.body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            if let Ok(v) = HeaderValue::from_str(&resp.content_type) {
                response.headers_mut().insert(header::CONTENT_TYPE, v);
            }
            response
        }
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": {
                    "message": format!("peer infer failed: {err}"),
                    "type": "peer_lm_remote_error",
                    "peer": peer.to_string(),
                }
            })),
        )
            .into_response(),
    }
}

fn forwarded_to_response(status: u16, headers: &HeaderMap, body: Bytes) -> Response {
    let mut response = Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::OK))
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());

    if let Some(ct) = headers.get(header::CONTENT_TYPE) {
        response.headers_mut().insert(header::CONTENT_TYPE, ct.clone());
    }
    response
}

pub async fn serve(listen: &str, state: GatewayState) -> Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!("gateway listening on http://{listen}");
    axum::serve(listener, app).await?;
    Ok(())
}
