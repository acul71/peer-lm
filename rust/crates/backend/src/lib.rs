use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct BackendClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendHealth {
    pub ok: bool,
    pub detail: String,
}

impl BackendClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .context("build reqwest client")?;
        Ok(Self { client, base_url })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn health(&self) -> BackendHealth {
        match self.client.get(format!("{}/health", self.base_url)).send().await {
            Ok(resp) if resp.status().is_success() => BackendHealth {
                ok: true,
                detail: format!("status {}", resp.status()),
            },
            Ok(resp) => BackendHealth {
                ok: false,
                detail: format!("status {}", resp.status()),
            },
            Err(err) => BackendHealth {
                ok: false,
                detail: err.to_string(),
            },
        }
    }

    pub async fn get_models_json(&self) -> Result<serde_json::Value> {
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await
            .context("GET /v1/models")?;
        let status = resp.status();
        let body = resp.bytes().await?;
        if !status.is_success() {
            return Err(anyhow!("backend /v1/models returned {status}: {}", String::from_utf8_lossy(&body)));
        }
        Ok(serde_json::from_slice(&body)?)
    }

    /// Forward a chat completions request; returns status, headers, and full body (non-stream or collected stream).
    pub async fn forward_chat_completions(
        &self,
        body: Bytes,
        stream: bool,
    ) -> Result<ForwardedResponse> {
        let mut req = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header(CONTENT_TYPE, "application/json")
            .body(body);

        if stream {
            req = req.header("Accept", "text/event-stream");
        }

        let resp = req.send().await.context("POST /v1/chat/completions")?;
        let status = resp.status().as_u16();
        let headers = filter_response_headers(resp.headers());
        let body = resp.bytes().await?;
        Ok(ForwardedResponse {
            status,
            headers,
            body,
        })
    }

    /// Stream chat completions SSE bytes from the backend.
    pub async fn stream_chat_completions(
        &self,
        body: Bytes,
    ) -> Result<(u16, HeaderMap, impl futures::Stream<Item = Result<Bytes, reqwest::Error>>)> {
        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header(CONTENT_TYPE, "application/json")
            .header("Accept", "text/event-stream")
            .body(body)
            .send()
            .await
            .context("POST /v1/chat/completions (stream)")?;

        let status = resp.status().as_u16();
        let headers = filter_response_headers(resp.headers());
        let stream = resp.bytes_stream().map(|r| r.map_err(Into::into));
        Ok((status, headers, stream))
    }
}

#[derive(Debug, Clone)]
pub struct ForwardedResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

fn filter_response_headers(src: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for name in ["content-type", "cache-control"] {
        if let Ok(hn) = HeaderName::from_bytes(name.as_bytes()) {
            if let Some(v) = src.get(&hn) {
                if let Ok(cloned) = HeaderValue::from_bytes(v.as_bytes()) {
                    out.insert(hn, cloned);
                }
            }
        }
    }
    out
}
