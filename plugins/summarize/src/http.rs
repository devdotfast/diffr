//! Outgoing HTTP through the standard WASI interface, available to external
//! components without a diffr-specific network import.
use diffr_plugin_sdk::anyhow::{self, anyhow};
use wasi::http::{outgoing_handler, types::*};
use wasi::io::streams::StreamError;

pub fn post(url: &str, key: &str, body: &str, timeout_ms: u64) -> anyhow::Result<(u16, Vec<u8>)> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| anyhow!("invalid endpoint URL"))?;
    let scheme = match scheme {
        "http" => Scheme::Http,
        "https" => Scheme::Https,
        _ => anyhow::bail!("endpoint must use http or https"),
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let headers = Fields::from_list(&[
        ("content-type".into(), b"application/json".to_vec()),
        ("content-length".into(), body.len().to_string().into_bytes()),
        ("x-goog-api-key".into(), key.as_bytes().to_vec()),
    ])
    .map_err(|e| anyhow!("HTTP headers: {e:?}"))?;
    let request = OutgoingRequest::new(headers);
    request
        .set_method(&Method::Post)
        .map_err(|_| anyhow!("HTTP method"))?;
    request
        .set_scheme(Some(&scheme))
        .map_err(|_| anyhow!("HTTP scheme"))?;
    request
        .set_authority(Some(authority))
        .map_err(|_| anyhow!("HTTP authority"))?;
    request
        .set_path_with_query(Some(&format!("/{path}")))
        .map_err(|_| anyhow!("HTTP path"))?;
    let outgoing = request.body().map_err(|_| anyhow!("HTTP body"))?;
    let options = RequestOptions::new();
    let timeout = Some(timeout_ms.saturating_mul(1_000_000));
    options
        .set_connect_timeout(timeout)
        .map_err(|_| anyhow!("connect timeout"))?;
    options
        .set_first_byte_timeout(timeout)
        .map_err(|_| anyhow!("response timeout"))?;
    options
        .set_between_bytes_timeout(timeout)
        .map_err(|_| anyhow!("read timeout"))?;
    let response = outgoing_handler::handle(request, Some(options))
        .map_err(|e| anyhow!("HTTP request: {e:?}"))?;
    {
        let stream = outgoing
            .write()
            .map_err(|_| anyhow!("HTTP output stream"))?;
        for chunk in body.as_bytes().chunks(4096) {
            stream
                .blocking_write_and_flush(chunk)
                .map_err(|e| anyhow!("HTTP write: {e:?}"))?;
        }
    }
    OutgoingBody::finish(outgoing, None).map_err(|e| anyhow!("HTTP finish: {e:?}"))?;
    response.subscribe().block();
    let response = response
        .get()
        .ok_or_else(|| anyhow!("HTTP response not ready"))?
        .map_err(|_| anyhow!("HTTP response already consumed"))?
        .map_err(|e| anyhow!("HTTP response: {e:?}"))?;
    let status = response.status();
    let incoming = response
        .consume()
        .map_err(|_| anyhow!("HTTP response body"))?;
    let mut bytes = Vec::new();
    {
        let stream = incoming
            .stream()
            .map_err(|_| anyhow!("HTTP input stream"))?;
        loop {
            match stream.blocking_read(64 * 1024) {
                Ok(chunk) => bytes.extend_from_slice(&chunk),
                Err(StreamError::Closed) => break,
                Err(error) => anyhow::bail!("HTTP read: {error:?}"),
            }
        }
    }
    let trailers = IncomingBody::finish(incoming);
    trailers.subscribe().block();
    trailers
        .get()
        .ok_or_else(|| anyhow!("HTTP trailers not ready"))?
        .map_err(|_| anyhow!("HTTP trailers already consumed"))?
        .map_err(|e| anyhow!("HTTP trailers: {e:?}"))?;
    Ok((status, bytes))
}
