use std::{future::IntoFuture, net::SocketAddr, sync::Arc};

use anyhow::{anyhow, Result};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    serve, Router,
};
use base64::{engine::general_purpose::STANDARD as base64, Engine};
use chrono::Utc;
use clap::{command, Parser};
use crossbeam_skiplist::SkipMap;
use futures_delay_queue::delay_queue;
use log::{error, info, warn};
use pyo3::{
    types::{PyBytes, PyModule},
    PyResult, Python,
};
use tokio::{
    net::TcpListener,
    select, spawn,
    task::yield_now,
    time::{Duration, Instant},
};

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// The address to bind to
    #[arg(long, env = "BIND_ADDR", default_value = "[::]:8080")]
    addr: SocketAddr,
    /// The number of entries to cache
    #[arg(long, env = "CACHE_SIZE", default_value = "128")]
    cache_size: usize,
}
const ONE_MINUTE: Duration = Duration::from_secs(60);
const FIFTEEN_MINUTES: Duration = Duration::from_secs(15 * 60);

#[pyo3_asyncio::tokio::main(flavor = "multi_thread")]
async fn main() -> PyResult<()> {
    pretty_env_logger::init();
    let Args { addr, cache_size } = Args::parse();

    initialize_python()?;

    let cache = Arc::new(SkipMap::new());
    let (gen_cache, inv_cache) = (cache.clone(), cache.clone());
    let (exp_sender, expr_reciever) = delay_queue::<Instant>();
    let cache_producer = spawn(async move {
        loop {
            yield_now().await;
            if gen_cache.len() >= cache_size {
                continue;
            }
            let Ok((expiry, data)) = generate_validation_data().await else {
                warn!("Failed to generate data, cache size: {}", gen_cache.len());
                continue;
            };
            gen_cache.insert(expiry, data);
            exp_sender.insert(expiry, expiry.duration_since(Instant::now()) - ONE_MINUTE);
            info!("Generated new data, cache size: {}", gen_cache.len());
        }
    });
    let cache_invalidator = spawn(async move {
        loop {
            yield_now().await;
            let Some(expiry) = expr_reciever.receive().await else {
                warn!("Failed to receive expiry, cache size: {}", inv_cache.len());
                continue;
            };
            inv_cache.remove(&expiry);
            info!("Evicted expired data, cache size: {}", inv_cache.len());
        }
    });

    info!("Listening on {}", addr);
    let listener = TcpListener::bind(addr).await?;
    let app = Router::new()
        .route("/", get(frontend))
        .route("/generate", get(serve_validation_data))
        .route("/LICENSE", get(|| async { include_str!("../LICENSE") }))
        .route(
            "/version",
            get(|| async { format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")) }),
        )
        .with_state(cache);
    info!("Starting server...");
    let server = serve(listener, app).into_future();
    select! {
        _ = cache_producer => {}
        _ = cache_invalidator => {}
        Err(e) = server => error!("Server error: {}", e),
        _ = tokio::signal::ctrl_c() => {
            info!("Shutting down...");
            return Ok(());
        }
    }
    Err(anyhow!("Server exited unexpectedly").into())
}

const IMD_APPLE_SERVICES: &[u8] = include_bytes!("IMDAppleServices");
const DATA_PLIST: &[u8] = include_bytes!("data.plist");

fn initialize_python() -> PyResult<()> {
    let py_mparser = include_str!("mparser.py");
    let py_jelly = include_str!("jelly.py");
    let py_nac = include_str!("nac.py");
    Python::with_gil(|py| -> PyResult<()> {
        PyModule::from_code(py, py_mparser, "mparser.py", "mparser")?;
        PyModule::from_code(py, py_jelly, "jelly.py", "jelly")?;
        let fake_data = PyBytes::new(py, DATA_PLIST);
        let binary = PyBytes::new(py, IMD_APPLE_SERVICES);
        let fake_data = PyModule::import(py, "plistlib")?.call_method1("loads", (fake_data,))?;
        let nac = PyModule::from_code(py, py_nac, "nac.py", "nac")?;
        nac.setattr("FAKE_DATA", fake_data)?;
        nac.setattr("BINARY", binary)?;
        Ok(())
    })
}

async fn serve_validation_data(
    State(cache): State<Arc<SkipMap<Instant, Box<str>>>>,
) -> Result<impl IntoResponse, StatusCode> {
    let (expiry, data) = {
        let entry = cache.pop_back().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        (entry.key().clone(), entry.value().clone())
    };
    info!("Serving data, cache size: {}", cache.len());
    if expiry.elapsed() != Duration::ZERO {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let expires = Utc::now() + expiry.elapsed();
    let mut headers = HeaderMap::new();
    headers.insert(
        "Expires",
        expires
            .to_rfc2822()
            .parse()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    Ok((headers, data))
}

async fn generate_validation_data() -> Result<(Instant, Box<str>)> {
    let expiry = Instant::now() + FIFTEEN_MINUTES;
    let data = Python::with_gil(|py| -> PyResult<_> {
        let nac = PyModule::import(py, "nac")?;
        let data = nac
            .call_method0("generate_validation_data")?
            .extract::<Vec<u8>>()?;
        Ok(data)
    })?;
    let data = base64.encode(data);
    Ok((expiry, data.into()))
}

async fn frontend() -> Html<String> {
    Html::from(format!(
        include_str!("index.html"),
        github_url = env!("CARGO_PKG_REPOSITORY")
    ))
}
