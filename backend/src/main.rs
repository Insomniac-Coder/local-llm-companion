//! companion-backend: local-first AI PC companion server (Stage 5).
//! Serves the React UI's local API (§57). Binds 127.0.0.1 only by default.

// Staged scaffold: modules for Stages 6–15 (agent tools, vision, docs,
// automation) exist ahead of their wiring. Allow dead code until each stage
// lands instead of drowning `cargo run` in 30 benign warnings.
#![allow(dead_code)]

mod agent;
mod agent_progress;
mod agent_runner;
mod api;
mod calibration;
mod cdp;
mod cpu_topology;
mod commands;
mod config;
mod daio;
mod documents;
mod downloads;
mod file_read;
mod generation;
mod hardware;
mod inference;
mod inspection_context;
mod llamaserver;
mod logbuf;
mod logfile;
mod metrics;
mod models;
mod outline;
mod permissions;
mod preview;
mod project_check;
mod recommend;
mod repo_index;
mod runtime_fit;
mod runtime_selection;
mod search;
mod settings;
mod shutdown;
mod speed_rule;
mod storage;
mod stream_split;
mod terminal;
mod tooling;
mod tools;
mod vision;
mod workspace;

use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        // A second copy of every record, kept in memory so a session export
        // can carry it to another machine to be read.
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(logbuf::global().clone()),
        )
        // And a third on disk (logs/companion.log in the data folder): the
        // console and the memory tail are both gone after a restart.
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(|| logfile::AppFileWriter),
        )
        .init();

    let cfg = config::AppConfig::from_env();
    if let Err(e) = std::fs::create_dir_all(&cfg.data_dir) {
        tracing::error!("cannot create data dir {}: {e}", cfg.data_dir.display());
        std::process::exit(1);
    }
    logfile::init(&cfg.data_dir.join("logs"));
    tracing::info!(data = %cfg.data_dir.display(), models = %cfg.models_dir.display(), "resolved application storage");

    let storage = match storage::Storage::open_persistent(&cfg.db_path()) {
        Ok(s) => {
            tracing::info!("opened database {}", cfg.db_path().display());
            s
        }
        Err(e) => {
            tracing::error!(
                "cannot open {}: {e}; startup stopped to protect persistent history. Check permissions or COMPANION_DATA_DIR.",
                cfg.db_path().display()
            );
            std::process::exit(1);
        }
    };

    let state = api::AppState::new_with_dirs(
        storage,
        cfg.models_dir.clone(),
        cfg.data_dir.join("attachments"),
    )
    .with_install_root(cfg.root.clone());

    // Register models found on disk (§9 layout), then seed demo only if empty
    // so the UI Model selector is never blank on first launch (§88).
    let (found, warnings) = models::scan_models_dir(&cfg.models_dir);
    for w in warnings {
        tracing::warn!("{w}");
    }
    {
        let mut mm = state.models.write().await;
        for m in found {
            if let Err(e) = mm.register(m) {
                tracing::warn!("skipping model: {e}");
            }
        }
        // No placeholder entry when the folder is empty: a listed model must be
        // a file that can be loaded. The setup screen explains how to add one.
        if mm.is_empty() {
            tracing::info!(
                "no models in {}; add a GGUF (models/modeldownloader.py) and the list updates on its next read",
                cfg.models_dir.display()
            );
        } else {
            tracing::info!(
                "registered {} model(s) from {}",
                mm.list().len(),
                cfg.models_dir.display()
            );
        }
    }

    // Each model's fit is searched ahead of its first load (owner decision
    // 2026-09-17), once the interface has had time to come up.
    api::spawn_fit_preparation(&state, std::time::Duration::from_secs(20));

    // Refuse non-loopback binds unless explicitly overridden (§54).
    if !(cfg.addr.starts_with("127.0.0.1:") || cfg.addr.starts_with("localhost:")) {
        tracing::warn!(
            "COMPANION_ADDR={} is not loopback; the API has no auth. Prefer 127.0.0.1.",
            cfg.addr
        );
    }

    // Stage 15: resource sampler (2 s cadence, 1 h ring).
    {
        let log = state.metrics.clone();
        let gens = state.generations.clone();
        let counter = state.agent_active.clone();
        let active: std::sync::Arc<dyn Fn() -> usize + Send + Sync> =
            std::sync::Arc::new(move || {
                let g = gens
                    .try_read()
                    .map(|t| t.current_id().is_some() as usize)
                    .unwrap_or(0);
                g + counter.load(std::sync::atomic::Ordering::Relaxed)
            });
        tokio::spawn(crate::metrics::sampler(log, active));
    }

    if !cfg.frontend_dir.join("index.html").is_file() {
        let start_script = if cfg!(windows) { ".\\run.ps1 (or run.bat)" } else { "./run.sh" };
        tracing::warn!(
            "frontend build not found at {}; start Companion with {start_script} or run `npm run build` in frontend/",
            cfg.frontend_dir.display()
        );
    }

    // One local process owns both the API and the built React application.
    // API routes are registered first; ServeDir is only the SPA fallback.
    let index = cfg.frontend_dir.join("index.html");
    let app = api::router(state.clone())
        .fallback_service(ServeDir::new(&cfg.frontend_dir).fallback(ServeFile::new(index)))
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async {
                if response
                    .headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map(|value| value.starts_with("text/html"))
                    .unwrap_or(false)
                {
                    response.headers_mut().insert(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static("no-store"),
                    );
                }
                response
            },
        ));
    let listener = tokio::net::TcpListener::bind(&cfg.addr)
        .await
        .expect("bind local API");
    tracing::info!(
        "local companion listening on http://{} (API + UI)",
        cfg.addr
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state))
        .await
        .expect("serve");
    tracing::info!("local companion exited cleanly");
}

/// Why the process is being asked to stop.
#[derive(Debug, Clone, Copy)]
enum StopRequest {
    CtrlC,
    WindowClosed,
    SigningOut,
    SystemShutdown,
    Terminate,
    TerminalClosed,
}

impl StopRequest {
    fn reason(self) -> &'static str {
        match self {
            Self::CtrlC => "Ctrl+C in its window",
            Self::WindowClosed => "its window was closed",
            Self::SigningOut => "Windows was signing out",
            Self::SystemShutdown => "Windows was shutting down or restarting",
            Self::Terminate => "it was asked to stop",
            Self::TerminalClosed => "its terminal was closed",
        }
    }

    /// The system ends the process a few seconds after these whether or not
    /// it has finished; what matters is recorded by then.
    fn ends_the_process(self) -> bool {
        matches!(
            self,
            Self::WindowClosed | Self::SigningOut | Self::SystemShutdown | Self::TerminalClosed
        )
    }
}

async fn shutdown_signal(state: api::AppState) {
    let request = stop_requested().await;
    let reason = request.reason();
    tracing::info!("stopping ({reason}): recording running tasks, then stopping the model server");
    state.shut_down(reason).await;
    if request.ends_the_process() {
        // Open connections would only hold the exit until the system ends
        // the process anyway.
        tracing::info!("local companion exited ({reason})");
        std::process::exit(0);
    }
}

/// Waits for Ctrl+C, and for the window closing, signing out or shutting
/// down (Windows) or SIGTERM and SIGHUP (unix). Only Ctrl+C used to be
/// heard: closing the window ended the process with nothing recorded.
async fn stop_requested() -> StopRequest {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "cannot listen for Ctrl+C");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let listen = |kind: SignalKind, what: &'static str| async move {
            match signal(kind) {
                Ok(mut listener) => {
                    listener.recv().await;
                }
                Err(error) => {
                    tracing::warn!(%error, "cannot listen for {what}");
                    std::future::pending::<()>().await;
                }
            }
        };
        tokio::select! {
            _ = ctrl_c => StopRequest::CtrlC,
            _ = listen(SignalKind::terminate(), "SIGTERM") => StopRequest::Terminate,
            _ = listen(SignalKind::hangup(), "SIGHUP") => StopRequest::TerminalClosed,
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows;
        // tokio holds the process open after these until the system's own
        // grace period ends, so the async side has a few seconds to act.
        let window_closed = async {
            match windows::ctrl_close() {
                Ok(mut listener) => {
                    listener.recv().await;
                }
                Err(error) => {
                    tracing::warn!(%error, "cannot listen for the window closing");
                    std::future::pending::<()>().await;
                }
            }
        };
        let signing_out = async {
            match windows::ctrl_logoff() {
                Ok(mut listener) => {
                    listener.recv().await;
                }
                Err(error) => {
                    tracing::warn!(%error, "cannot listen for signing out");
                    std::future::pending::<()>().await;
                }
            }
        };
        let shutting_down = async {
            match windows::ctrl_shutdown() {
                Ok(mut listener) => {
                    listener.recv().await;
                }
                Err(error) => {
                    tracing::warn!(%error, "cannot listen for shutdown");
                    std::future::pending::<()>().await;
                }
            }
        };
        let ctrl_break = async {
            match windows::ctrl_break() {
                Ok(mut listener) => {
                    listener.recv().await;
                }
                Err(error) => {
                    tracing::warn!(%error, "cannot listen for Ctrl+Break");
                    std::future::pending::<()>().await;
                }
            }
        };
        tokio::select! {
            _ = ctrl_c => StopRequest::CtrlC,
            _ = ctrl_break => StopRequest::CtrlC,
            _ = window_closed => StopRequest::WindowClosed,
            _ = signing_out => StopRequest::SigningOut,
            _ = shutting_down => StopRequest::SystemShutdown,
        }
    }
}
