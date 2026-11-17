use anyhow::Result;
use clap::Parser;
use peer_lm_mock_backend::{router, MockState};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "peer-lm-mock", about = "Tiny OpenAI-compatible mock backend")]
struct Args {
    /// Listen address
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: String,

    /// Model id advertised on /v1/models
    #[arg(long, default_value = "peer-lm-mock")]
    model: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let args = Args::parse();
    let state = MockState {
        model_id: args.model,
    };
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!("peer-lm-mock listening on http://{}", args.listen);
    axum::serve(listener, app).await?;
    Ok(())
}
