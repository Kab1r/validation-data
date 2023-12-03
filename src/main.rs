use std::net::SocketAddr;

use axum::{http::StatusCode, response::Html, routing::get, serve, Router};
use base64::{engine::general_purpose::STANDARD as base64, Engine};
use clap::{command, Parser};
use log::info;
use pyo3::{
    types::{PyBytes, PyModule},
    PyResult, Python,
};
use tokio::net::TcpListener;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    #[arg(long, env = "BIND_ADDR", default_value = "[::]:8080")]
    addr: SocketAddr,
}

#[pyo3_asyncio::tokio::main(flavor = "multi_thread")]
async fn main() -> PyResult<()> {
    pretty_env_logger::init();
    let Args { addr } = Args::parse();

    initialize_python()?;

    info!("Listening on {}", addr);
    let listener = TcpListener::bind(addr).await?;
    let app = Router::new()
        .route("/", get(frontend))
        .route("/generate", get(generate_validation_data))
        .route("/LICENSE", get(|| async { include_str!("../LICENSE") }))
        .route(
            "/version",
            get(|| async { format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")) }),
        );
    info!("Starting server...");
    serve(listener, app).await?;
    Ok(())
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

async fn generate_validation_data() -> Result<String, StatusCode> {
    let data = Python::with_gil(|py| -> PyResult<_> {
        let nac = PyModule::import(py, "nac")?;
        let data = nac
            .call_method0("generate_validation_data")?
            .extract::<Vec<u8>>()?;
        Ok(data)
    })
    .map_err(|_pyerr| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(base64.encode(data))
}

async fn frontend() -> Html<String> {
    Html::from(format!(
        include_str!("index.html"),
        github_url = env!("CARGO_PKG_REPOSITORY")
    ))
}
