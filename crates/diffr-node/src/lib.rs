//! Native entry points for `diffr/api`. Search implementation follows the
//! reviewed integration-output contract; these exports deliberately fail until
//! that implementation exists.
use napi::{Error, Result};
use napi_derive::napi;
use serde_json::Value;

#[napi]
pub async fn hydrate(_scope: Value, _hits: Value) -> Result<Value> {
    Err(Error::from_reason("diffr/api: hydrate is not implemented"))
}

#[napi]
pub async fn postprocess(_scope: Value, _selected: Value, _options: Option<Value>) -> Result<Value> {
    Err(Error::from_reason("diffr/api: postprocess is not implemented"))
}
