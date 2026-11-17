use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::time::Duration;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

pub const MOCK_MODEL_ID: &str = "peer-lm-mock";

#[derive(Clone, Debug)]
pub struct MockState {
    pub model_id: String,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            model_id: MOCK_MODEL_ID.to_string(),
        }
    }
}

pub fn router(state: MockState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "engine": "peer-lm-mock" }))
}

async fn list_models(State(state): State<MockState>) -> Json<Value> {
    Json(json!({
        "object": "list",
        "data": [{
            "id": state.model_id,
            "object": "model",
            "created": 0,
            "owned_by": "peer-lm"
        }]
    }))
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    #[serde(default)]
    messages: Vec<ChatMessage>,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: Value,
}

fn message_text(msg: &ChatMessage) -> String {
    match &msg.content {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn canned_reply(req: &ChatRequest) -> String {
    let last = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(message_text)
        .unwrap_or_else(|| "(empty)".into());
    let preview: String = last.chars().take(80).collect();
    format!(
        "peer-lm-mock: got {} message(s). Last user text: {preview}",
        req.messages.len()
    )
}

async fn chat_completions(State(state): State<MockState>, Json(req): Json<ChatRequest>) -> Response {
    let model = req
        .model
        .clone()
        .unwrap_or_else(|| state.model_id.clone());
    let content = canned_reply(&req);
    let id = format!("chatcmpl-{}", Uuid::new_v4());

    if req.stream {
        return Sse::new(sse_events(id, model, content))
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    Json(json!({
        "id": id,
        "object": "chat.completion",
        "created": Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0
        }
    }))
    .into_response()
}

#[derive(Serialize)]
struct StreamChunk<'a> {
    id: &'a str,
    object: &'a str,
    created: i64,
    model: &'a str,
    choices: Vec<StreamChoice<'a>>,
}

#[derive(Serialize)]
struct StreamChoice<'a> {
    index: u32,
    delta: StreamDelta<'a>,
    finish_reason: Option<&'a str>,
}

#[derive(Serialize)]
struct StreamDelta<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
}

fn sse_events(
    id: String,
    model: String,
    content: String,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let created = Utc::now().timestamp();
    let mut events: Vec<String> = Vec::new();

    let role_chunk = StreamChunk {
        id: &id,
        object: "chat.completion.chunk",
        created,
        model: &model,
        choices: vec![StreamChoice {
            index: 0,
            delta: StreamDelta {
                role: Some("assistant"),
                content: None,
            },
            finish_reason: None,
        }],
    };
    events.push(serde_json::to_string(&role_chunk).unwrap());

    let words: Vec<&str> = content.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let piece = if i + 1 == words.len() {
            (*w).to_string()
        } else {
            format!("{w} ")
        };
        let chunk = StreamChunk {
            id: &id,
            object: "chat.completion.chunk",
            created,
            model: &model,
            choices: vec![StreamChoice {
                index: 0,
                delta: StreamDelta {
                    role: None,
                    content: Some(&piece),
                },
                finish_reason: None,
            }],
        };
        events.push(serde_json::to_string(&chunk).unwrap());
    }

    let end = StreamChunk {
        id: &id,
        object: "chat.completion.chunk",
        created,
        model: &model,
        choices: vec![StreamChoice {
            index: 0,
            delta: StreamDelta {
                role: None,
                content: None,
            },
            finish_reason: Some("stop"),
        }],
    };
    events.push(serde_json::to_string(&end).unwrap());
    events.push("[DONE]".to_string());

    stream::unfold(events.into_iter(), |mut iter| async move {
        let next = iter.next()?;
        tokio::time::sleep(Duration::from_millis(10)).await;
        Some((Ok(Event::default().data(next)), iter))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_string(body: Body) -> String {
        let bytes = body.collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn health_ok() {
        let app = router(MockState::default());
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let body = body_string(res.into_body()).await;
        assert!(body.contains("peer-lm-mock"));
    }

    #[tokio::test]
    async fn lists_model() {
        let app = router(MockState::default());
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let body = body_string(res.into_body()).await;
        assert!(body.contains(MOCK_MODEL_ID));
    }

    #[tokio::test]
    async fn chat_completion_non_stream() {
        let app = router(MockState::default());
        let payload = serde_json::json!({
            "model": "peer-lm-mock",
            "messages": [{"role":"user","content":"hello tests"}]
        });
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let body = body_string(res.into_body()).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        let content = v["choices"][0]["message"]["content"].as_str().unwrap();
        assert!(content.contains("hello tests"));
    }

    #[tokio::test]
    async fn chat_completion_stream_sse() {
        let app = router(MockState::default());
        let payload = serde_json::json!({
            "model": "peer-lm-mock",
            "stream": true,
            "messages": [{"role":"user","content":"stream me"}]
        });
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let ct = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.contains("text/event-stream"), "ct={ct}");
        let body = body_string(res.into_body()).await;
        assert!(body.contains("data:"));
        assert!(body.contains("[DONE]"));
        assert!(body.contains("stream") || body.contains("peer-lm-mock"));
    }
}
