//! Bridges GPUI's image loading onto the app's shared reqwest client so that
//! `img("https://…")` works for avatars, attachments and emoji.

use futures::future::BoxFuture;
use gpui::http_client::{AsyncBody, HttpClient, Request, Response, Url, http::HeaderValue};

pub struct ReqwestClient {
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
}

impl ReqwestClient {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("native-client/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client");
        Self { client, runtime }
    }
}

impl HttpClient for ReqwestClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(&self, req: Request<AsyncBody>) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let client = self.client.clone();
        let runtime = self.runtime.clone();
        Box::pin(async move {
            let method = reqwest::Method::from_bytes(req.method().as_str().as_bytes())?;
            let url = req.uri().to_string();
            // reqwest needs a Tokio context; GPUI drives this future on its own executor.
            let (status, bytes) = runtime
                .spawn(async move {
                    let resp = client.request(method, url).send().await?;
                    let status = resp.status().as_u16();
                    let bytes = resp.bytes().await?;
                    anyhow::Ok((status, bytes))
                })
                .await??;
            Ok(Response::builder().status(status).body(AsyncBody::from_bytes(bytes))?)
        })
    }
}
