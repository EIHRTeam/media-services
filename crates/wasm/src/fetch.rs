//! The browser's half of `SignTransport`: `fetch` against the signing service.
//!
//! Note the `?Send` on the impl. On wasm the futures are backed by `JsFuture`,
//! which cannot cross threads, so c2pa relaxes the bound there — and so must we,
//! or the transport is rejected for not being `Send`.

use provenance::{Error, SignTransport, SignerInfo};
use wasm_bindgen::{JsCast, JsValue};

/// A failure carrying the HTTP status, so `SignerUnauthorized` can be told from
/// a generic network error.
fn status_error(status: u16, body: &[u8]) -> Error {
    let message = String::from_utf8_lossy(body).trim().to_string();
    match status {
        401 | 403 => Error::SignerUnauthorized,
        _ => Error::SignerRejected { status, message },
    }
}

#[derive(Clone)]
pub struct FetchTransport {
    endpoint: String,
    token: String,
    fetch: Option<js_sys::Function>,
}

impl FetchTransport {
    pub fn new(endpoint: impl Into<String>, token: impl Into<String>) -> Self {
        // A trailing slash would produce `//signer`.
        let endpoint = endpoint.into();
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            token: token.into(),
            fetch: None,
        }
    }

    pub fn with_fetch(mut self, fetch: wasm_bindgen::JsValue) -> Result<Self, JsValue> {
        self.fetch = if fetch.is_undefined() || fetch.is_null() {
            None
        } else {
            Some(
                fetch
                    .dyn_into()
                    .map_err(|_| crate::error::invalid_argument("fetch must be a function"))?,
            )
        };
        Ok(self)
    }

    async fn send(&self, path: &str, body: Option<Vec<u8>>) -> Result<(u16, Vec<u8>), Error> {
        let url = format!("{}{path}", self.endpoint);

        let headers = web_sys::Headers::new().map_err(|e| network_error(&e))?;
        headers
            .set("authorization", &format!("Bearer {}", self.token))
            .map_err(|e| network_error(&e))?;
        if body.is_some() {
            headers
                .set("content-type", "application/octet-stream")
                .map_err(|e| network_error(&e))?;
        }

        let init = web_sys::RequestInit::new();
        init.set_method(if body.is_some() { "POST" } else { "GET" });
        init.set_mode(web_sys::RequestMode::Cors);
        init.set_headers(&headers);
        if let Some(bytes) = &body {
            init.set_body(&js_sys::Uint8Array::from(bytes.as_slice()));
        }

        let request =
            web_sys::Request::new_with_str_and_init(&url, &init).map_err(|e| network_error(&e))?;

        // `globalThis.fetch` rather than `window.fetch`: the same module has to
        // run in a page, a worker and Node, and only the first of those has a
        // `window`. `Request`, `Headers` and `Response` are globals in all three.
        let global = js_sys::global();
        let fetch = self
            .fetch
            .clone()
            .or_else(|| {
                js_sys::Reflect::get(&global, &JsValue::from_str("fetch"))
                    .ok()
                    .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
            })
            .ok_or_else(|| {
                Error::Network("no fetch in this environment; Node needs 18 or newer".into())
            })?;

        let response = fetch
            .call1(&global, &request)
            .map_err(|e| network_error(&e))?;
        let response = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(response))
            .await
            // The fetch rejection quotes the URL, which is why anything carrying
            // a secret is scrubbed before it reaches a caller.
            .map_err(|e| network_error(&e))?;
        let response: web_sys::Response = response
            .dyn_into()
            .map_err(|_| Error::Network("fetch did not return a Response".into()))?;

        let status = response.status();
        let buffer = wasm_bindgen_futures::JsFuture::from(
            response.array_buffer().map_err(|e| network_error(&e))?,
        )
        .await
        .map_err(|e| network_error(&e))?;
        let body = js_sys::Uint8Array::new(&buffer).to_vec();

        Ok((status, body))
    }
}

fn network_error(value: &JsValue) -> Error {
    let message = value
        .as_string()
        .or_else(|| {
            js_sys::Reflect::get(value, &JsValue::from_str("message"))
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| "network request failed".to_string());
    Error::Network(message)
}

#[async_trait::async_trait(?Send)]
impl SignTransport for FetchTransport {
    async fn signer_info(&self) -> Result<SignerInfo, Error> {
        let (status, body) = self.send("/signer", None).await?;
        if !(200..300).contains(&status) {
            return Err(status_error(status, &body));
        }
        serde_json::from_slice(&body)
            .map_err(|e| Error::InvalidSignerInfo(format!("GET /signer did not return JSON: {e}")))
    }

    async fn sign_sig_structure(&self, tbs: Vec<u8>) -> Result<Vec<u8>, Error> {
        let (status, body) = self.send("/sign", Some(tbs)).await?;
        if !(200..300).contains(&status) {
            return Err(status_error(status, &body));
        }
        // COSE wants raw r‖s, and the service is built to produce exactly that.
        if body.len() != 64 {
            return Err(Error::Network(format!(
                "the signing service returned {} bytes; an ES256 signature is 64",
                body.len()
            )));
        }
        Ok(body)
    }
}
