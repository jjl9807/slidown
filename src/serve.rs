use crate::{
    Input, build,
    render::{Compiler, Deck},
};
use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{OriginalUri, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Serialize)]
struct Status {
    revision: String,
    error: Option<String>,
    warnings: Vec<String>,
}

struct Preview {
    files: BTreeMap<String, Vec<u8>>,
    status: Status,
}
type Shared = Arc<RwLock<Preview>>;

pub async fn run(input: Input, host: IpAddr, port: u16, open: bool) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(host, port))
        .await
        .with_context(|| format!("listen on {host}:{port}"))?;
    let address = listener.local_addr()?;
    let state = Arc::new(RwLock::new(Preview {
        files: BTreeMap::new(),
        status: Status {
            revision: "initial".into(),
            error: None,
            warnings: vec![],
        },
    }));
    let watch_state = state.clone();
    let running = Arc::new(AtomicBool::new(true));
    let watch_running = running.clone();
    // Rendering and file reads run away from the async HTTP executor.
    let worker = tokio::task::spawn_blocking(move || watch(input, watch_state, watch_running));
    let app = Router::new()
        .route("/__slidown/status", get(status))
        .route("/__slidown/reload.js", get(reload))
        .fallback(get(asset))
        .with_state(state);
    let url = format!("http://{address}/");
    println!("Preview: {url}\nWatching outline and local images; Ctrl-C to stop.");
    if open {
        open_browser(&url);
    }
    let shutdown_running = running.clone();
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown_running.store(false, Ordering::Relaxed);
        })
        .await;
    running.store(false, Ordering::Relaxed);
    let outcome = worker.await.context("preview worker failed")??;
    if let Some(artifacts) = outcome.artifacts {
        artifacts.cleanup()?;
        println!("Preview stopped; generated artifacts cleaned up.");
    }
    result.context("preview server failed")
}

#[derive(Default)]
struct WatchOutcome {
    artifacts: Option<build::OutputDirectory>,
}

fn watch(input: Input, state: Shared, running: Arc<AtomicBool>) -> Result<WatchOutcome> {
    let mut compiler = Compiler::new(false)?;
    let mut outcome = WatchOutcome::default();
    let mut signatures = BTreeMap::new();
    let mut first = true;
    while running.load(Ordering::Relaxed) {
        let current = fingerprints(&compiler);
        if first || current != signatures {
            first = false;
            // A short debounce also accommodates editors that save via rename.
            std::thread::sleep(Duration::from_millis(120));
            if !running.load(Ordering::Relaxed) {
                break;
            }
            let mut before = fingerprints(&compiler);
            let outline = std::path::absolute(&input.outline)?;
            before.insert(
                outline.clone(),
                std::fs::read(&outline)
                    .ok()
                    .map(|bytes| build::digest(&bytes)),
            );
            let result = compiler.compile(&input.outline).and_then(|deck| {
                outcome.artifacts = Some(build::publish(
                    &deck,
                    &input.output,
                    &compiler.dependencies,
                )?);
                Ok(deck)
            });
            signatures = fingerprints(&compiler);
            // Preserve pre-build fingerprints: edits made during rendering need another build.
            for (path, signature) in before {
                if let Some(current) = signatures.get_mut(&path) {
                    *current = signature;
                }
            }
            let mut preview = state.write().expect("preview lock");
            match result {
                Ok(Deck {
                    files,
                    warnings,
                    slides,
                }) => {
                    let mut bytes = Vec::new();
                    for (name, contents) in &files {
                        bytes.extend_from_slice(name.as_bytes());
                        bytes.extend_from_slice(contents);
                    }
                    preview.status = Status {
                        revision: build::digest(&bytes),
                        error: None,
                        warnings,
                    };
                    preview.files = files;
                    println!("Built {slides} slides");
                    for warning in &preview.status.warnings {
                        eprintln!("warning: {warning}");
                    }
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    eprintln!("error: {message}");
                    preview.status.error = Some(message);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Ok(outcome)
}

// Content fingerprints detect same-size edits, deleted files and atomic saves on all platforms.
fn fingerprints(compiler: &Compiler) -> BTreeMap<PathBuf, Option<String>> {
    compiler
        .dependencies
        .iter()
        .map(|p| {
            (
                p.clone(),
                std::fs::read(p).ok().map(|bytes| build::digest(&bytes)),
            )
        })
        .collect()
}

async fn status(State(state): State<Shared>) -> impl IntoResponse {
    let status = state.read().expect("preview lock").status.clone();
    ([(header::CACHE_CONTROL, "no-store")], axum::Json(status))
}

async fn reload() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        include_str!("../assets/reload.js"),
    )
}

async fn asset(State(state): State<Shared>, OriginalUri(uri): OriginalUri) -> Response {
    let path = percent_encoding::percent_decode_str(uri.path()).decode_utf8_lossy();
    let name = if path == "/" {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };
    let preview = state.read().expect("preview lock");
    // Serve only the current generated file set, never arbitrary files in the output directory.
    let bytes = match preview.files.get(name) {
        Some(bytes) => bytes.clone(),
        None if name == "index.html" => b"<!doctype html><html><head><meta charset=\"utf-8\"><title>Slidown preview</title></head><body><p>Waiting for a successful build...</p></body></html>".to_vec(),
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    let bytes = if name == "index.html" {
        let injection = format!(
            "<script>window.__slidownRevision={};</script><script src=\"/__slidown/reload.js\"></script>",
            serde_json::to_string(&preview.status.revision).unwrap()
        );
        String::from_utf8(bytes)
            .unwrap()
            .replace("</body>", &(injection + "</body>"))
            .into_bytes()
    } else {
        bytes
    };
    let mime = match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response()
}

fn open_browser(url: &str) {
    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let result: std::io::Result<std::process::Child> =
        Err(std::io::Error::other("unsupported platform"));
    if let Err(error) = result {
        eprintln!("warning: could not open browser: {error}");
    }
}
