use synapse_server::Config;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if std::env::var("SYNAPSE_LOG_FORMAT").is_ok_and(|v| v.eq_ignore_ascii_case("json")) {
        builder.json().init();
    } else {
        builder.init();
    }
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("configuration error: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = synapse_server::serve(cfg).await {
        tracing::error!("server error: {e}");
        std::process::exit(1);
    }
}
