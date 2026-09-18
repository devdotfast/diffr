//! Node-API entry points delegate to the shared Rust search engine.
use napi::{Error, Result};
use napi_derive::napi;
use serde_json::Value;

#[napi]
pub async fn hydrate(scope: Value, hits: Value) -> Result<Value> {
    difftastic::search::hydrate(scope, hits)
        .map_err(|error| Error::from_reason(format!("{error:#}")))
}

#[napi]
pub async fn postprocess(scope: Value, selected: Value, options: Option<Value>) -> Result<Value> {
    difftastic::search::postprocess(scope, selected, options)
        .map_err(|error| Error::from_reason(format!("{error:#}")))
}
