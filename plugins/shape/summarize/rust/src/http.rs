//! Async HTTP and retry delays through WASI.
use std::time::Duration;

pub async fn post(
    url: &str,
    headers: &[(&'static str, String)],
    body: &str,
    timeout_ms: u64,
) -> anyhow::Result<(u16, Vec<u8>)> {
    use http_body_util::BodyExt;
    use wasip3::http_compat::{
        http_from_wasi_response, http_into_wasi_request, RequestOptionsExtension,
    };
    let options = wasip3::http::types::RequestOptions::new();
    let timeout = Some(timeout_ms.saturating_mul(1_000_000));
    options
        .set_connect_timeout(timeout)
        .map_err(|_| anyhow::anyhow!("connect timeout"))?;
    options
        .set_first_byte_timeout(timeout)
        .map_err(|_| anyhow::anyhow!("response timeout"))?;
    options
        .set_between_bytes_timeout(timeout)
        .map_err(|_| anyhow::anyhow!("read timeout"))?;
    let mut request = ::http::Request::post(url)
        .header("content-length", body.len())
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let request =
        request
            .extension(RequestOptionsExtension(options))
            .body(http_body_util::Full::new(bytes::Bytes::copy_from_slice(
                body.as_bytes(),
            )))?;
    let request =
        http_into_wasi_request(request).map_err(|e| anyhow::anyhow!("HTTP request: {e:?}"))?;
    let response = wasip3::http::client::send(request)
        .await
        .map_err(|e| anyhow::anyhow!("HTTP response: {e:?}"))?;
    let response =
        http_from_wasi_response(response).map_err(|e| anyhow::anyhow!("HTTP response: {e:?}"))?;
    let status = response.status().as_u16();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|e| anyhow::anyhow!("HTTP body: {e:?}"))?;
    Ok((status, body.to_bytes().to_vec()))
}

pub async fn sleep(duration: Duration) {
    wasip3::clocks::monotonic_clock::wait_for(duration.as_nanos().min(u64::MAX as u128) as u64)
        .await;
}
