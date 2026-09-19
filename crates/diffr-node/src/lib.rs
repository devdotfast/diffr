//! JSON conversion lives at the binding boundary; Rust search uses typed inputs.
//! Synchronous diff/storage work runs on N-API's blocking worker pool.
use difftastic::search::{configured_session, Options};
use napi::{bindgen_prelude::spawn_blocking, Error, Result};
use napi_derive::napi;
use serde_json::Value;

fn error(error: impl std::fmt::Display) -> Error {
    Error::from_reason(error.to_string())
}

#[napi]
pub async fn hydrate(scope: Value, hits: Value) -> Result<Value> {
    spawn_blocking(move || {
        let scope = serde_json::from_value(scope).map_err(error)?;
        let hits = serde_json::from_value(hits).map_err(error)?;
        let mut session =
            configured_session(scope, Options::default()).map_err(|e| error(format!("{e:#}")))?;
        let results = session.hydrate(hits).map_err(|e| error(format!("{e:#}")))?;
        serde_json::to_value(results).map_err(error)
    })
    .await
    .map_err(error)?
}

#[napi]
pub async fn postprocess(scope: Value, selected: Value, options: Option<Value>) -> Result<Value> {
    spawn_blocking(move || {
        let scope = serde_json::from_value(scope).map_err(error)?;
        let selected = serde_json::from_value(selected).map_err(error)?;
        let options = options
            .map(serde_json::from_value)
            .transpose()
            .map_err(error)?
            .unwrap_or_default();
        let mut session =
            configured_session(scope, options).map_err(|e| error(format!("{e:#}")))?;
        let results = session
            .postprocess(selected)
            .map_err(|e| error(format!("{e:#}")))?;
        serde_json::to_value(results).map_err(error)
    })
    .await
    .map_err(error)?
}
