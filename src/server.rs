//! One-workspace HTTP adapter over the file iterator.
use crate::config::{Config, Params};
use crate::git::{Comparison, DiffSession, FileChange, FileParams, Operand, Result};
use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use clap::{Arg, Command};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    convert::Infallible,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tower_http::services::ServeDir;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffRequest {
    before: Operand,
    after: Operand,
    #[serde(default)]
    files: FileParams,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    Start {
        version: u32,
        before: Operand,
        after: Operand,
        total: usize,
    },
    File {
        file: FileChange,
        diff: Value,
    },
    FileError {
        file: FileChange,
        message: String,
    },
    Complete {
        succeeded: usize,
        failed: usize,
    },
    Error {
        message: String,
    },
}

fn line(event: Event) -> std::result::Result<String, Infallible> {
    let mut line = serde_json::to_string(&event).expect("serializable stream event");
    line.push('\n');
    Ok(line)
}

struct Server {
    workspace: PathBuf,
    params: Arc<Params>,
}

async fn diff(State(server): State<Arc<Server>>, Json(request): Json<DiffRequest>) -> Response {
    let prepared = tokio::task::spawn_blocking(move || {
        DiffSession::open(
            &server.workspace,
            Comparison {
                before: request.before,
                after: request.after,
            },
            Arc::clone(&server.params),
            &request.files,
        )
    })
    .await;
    let mut session = match prepared {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": error.to_string()})),
            )
                .into_response()
        }
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": error.to_string()})),
            )
                .into_response()
        }
    };
    let stream = async_stream::stream! {
        yield line(Event::Start { version: 1, before: session.comparison.before.clone(), after: session.comparison.after.clone(), total: session.remaining() });
        let mut succeeded = 0;
        let mut failed = 0;
        while session.remaining() > 0 {
            // No task for the next file exists until the body is polled again.
            let next = tokio::task::spawn_blocking(move || {
                let (file, result) = session.next().expect("remaining file");
                let event = match result {
                    Ok(diff) => Event::File { file, diff: diff.domain_json() },
                    Err(error) => Event::FileError { file, message: error.to_string() },
                };
                (session, event)
            }).await;
            let (remaining, event) = match next {
                Ok(next) => next,
                Err(error) => {
                    yield line(Event::Error { message: error.to_string() });
                    return;
                }
            };
            session = remaining;
            match &event {
                Event::File { .. } => succeeded += 1,
                Event::FileError { .. } => failed += 1,
                _ => unreachable!(),
            }
            yield line(event);
        }
        yield line(Event::Complete { succeeded, failed });
    };
    (
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

pub(crate) fn run() -> Result<()> {
    let args = Command::new("difft serve")
        .arg(Arg::new("repo").long("repo").required(true))
        .arg(Arg::new("config").long("config"))
        .arg(
            Arg::new("listen")
                .long("listen")
                .default_value("127.0.0.1:4176")
                .value_parser(clap::value_parser!(SocketAddr)),
        )
        .arg(Arg::new("web-root").long("web-root"))
        .get_matches_from(
            std::iter::once(std::ffi::OsString::from("difft serve"))
                .chain(std::env::args_os().skip(2)),
        );
    let address = *args.get_one::<SocketAddr>("listen").unwrap();
    if !address.ip().is_loopback() {
        return Err("the local server must listen on a loopback address".into());
    }
    let repo = git2::Repository::discover(args.get_one::<String>("repo").unwrap())?;
    let workspace = repo
        .workdir()
        .ok_or("server requires a working tree for attributes")?
        .to_path_buf();
    let params = Arc::new(
        Config::load(&workspace, args.get_one::<String>("config").map(Path::new))?.compile()?,
    );
    let state = Arc::new(Server { workspace, params });
    let mut app = Router::new().route("/diff", post(diff)).with_state(state);
    if let Some(root) = args.get_one::<String>("web-root") {
        app = app.fallback_service(ServeDir::new(root));
    }
    tokio::runtime::Runtime::new()?.block_on(async {
        let listener = tokio::net::TcpListener::bind(address).await?;
        eprintln!("Listening on http://{}", listener.local_addr()?);
        axum::serve(listener, app).await
    })?;
    Ok(())
}
