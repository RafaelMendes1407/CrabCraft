//! Explicit requests to a separately installed, local Ollama service. Never pulls models.
use serde_json::{Value, json};
use std::{net::IpAddr, time::Duration};

const MAX_REPLY: usize = 1024 * 1024;
pub const MAX_CONTEXT: usize = 32 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Endpoint,
    Model,
    Network,
    Status(u16),
    Limit,
    Invalid,
    RemoteModel,
}

pub fn endpoint(text: &str) -> Result<reqwest::Url, Error> {
    if text.len() > 2048 {
        return Err(Error::Endpoint);
    }
    let mut url = reqwest::Url::parse(text).map_err(|_| Error::Endpoint)?;
    if url.host_str() == Some("localhost") {
        url.set_host(Some("127.0.0.1"))
            .map_err(|_| Error::Endpoint)?;
    }
    let local = url
        .host_str()
        .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback());
    if !local
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(Error::Endpoint);
    }
    Ok(url)
}
fn model_name(model: &str) -> Result<(), Error> {
    if model.is_empty()
        || model.len() > 256
        || !model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/:".contains(&b))
    {
        return Err(Error::Model);
    }
    if model
        .split(':')
        .next_back()
        .is_some_and(|tag| tag == "cloud" || tag.ends_with("-cloud"))
    {
        return Err(Error::RemoteModel);
    }
    Ok(())
}
fn remote(reply: &Value) -> bool {
    ["remote_host", "remote_model"].iter().any(|key| {
        reply
            .get(key)
            .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
    })
}
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: reqwest::Url,
}
impl Client {
    pub fn new(url: &str) -> Result<Self, Error> {
        let base = endpoint(url)?;
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| Error::Network)?;
        Ok(Self { http, base })
    }
    async fn request(&self, path: &str, body: Option<Value>) -> Result<Value, Error> {
        let url = self.base.join(path).map_err(|_| Error::Endpoint)?;
        let request = match body {
            Some(body) => self.http.post(url).json(&body),
            None => self.http.get(url),
        };
        let mut response = request.send().await.map_err(|_| Error::Network)?;
        if !response.status().is_success() {
            return Err(Error::Status(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_REPLY as u64)
        {
            return Err(Error::Limit);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
            if chunk.len() > MAX_REPLY.saturating_sub(bytes.len()) {
                return Err(Error::Limit);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)
    }
    pub async fn models(&self) -> Result<Vec<String>, Error> {
        let reply = self.request("api/tags", None).await?;
        let models = reply
            .get("models")
            .and_then(Value::as_array)
            .ok_or(Error::Invalid)?;
        if models.len() > 512 {
            return Err(Error::Limit);
        }
        let mut names = Vec::new();
        for model in models {
            let name = model
                .get("name")
                .and_then(Value::as_str)
                .ok_or(Error::Invalid)?;
            if model_name(name).is_ok()
                && !remote(model)
                && model
                    .get("size")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| n > 0)
            {
                names.push(name.to_owned());
            }
        }
        names.sort();
        names.dedup();
        Ok(names)
    }
    async fn local_model(&self, model: &str) -> Result<(), Error> {
        model_name(model)?;
        let reply = self
            .request("api/show", Some(json!({"model":model})))
            .await?;
        if remote(&reply) {
            return Err(Error::RemoteModel);
        }
        if reply.pointer("/details/format").and_then(Value::as_str) != Some("gguf")
            || !reply.get("model_info").is_some_and(Value::is_object)
        {
            return Err(Error::RemoteModel);
        }
        Ok(())
    }
    pub async fn chat(
        &self,
        model: &str,
        messages: Vec<Value>,
        schema: Option<Value>,
    ) -> Result<String, Error> {
        if messages.is_empty() || messages.len() > 24 {
            return Err(Error::Limit);
        }
        let bytes = serde_json::to_vec(&messages).map_err(|_| Error::Invalid)?;
        if bytes.len() > MAX_CONTEXT * 2 {
            return Err(Error::Limit);
        }
        self.local_model(model).await?;
        let mut body = json!({"model":model,"messages":messages,"stream":false,"options":{"temperature":0,"num_predict":512,"num_ctx":8192},"keep_alive":"5m"});
        if let Some(schema) = schema {
            body["format"] = schema;
        }
        let reply = self.request("api/chat", Some(body)).await?;
        if remote(&reply) {
            return Err(Error::RemoteModel);
        }
        if reply.get("done").and_then(Value::as_bool) != Some(true) {
            return Err(Error::Invalid);
        }
        let text = reply
            .pointer("/message/content")
            .and_then(Value::as_str)
            .ok_or(Error::Invalid)?;
        if text.len() > MAX_CONTEXT || text.contains('\0') {
            return Err(Error::Limit);
        }
        Ok(text.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_loopback_without_credentials_paths_or_cloud_models() {
        for url in [
            "http://127.0.0.1:11434",
            "http://[::1]:1234",
            "http://localhost:11434",
        ] {
            assert!(endpoint(url).is_ok(), "{url}");
        }
        for url in [
            "http://example.org",
            "http://192.168.0.1",
            "http://127.0.0.1/api",
            "http://u:p@127.0.0.1",
            "http://127.0.0.1?q=1",
            "ftp://127.0.0.1",
        ] {
            assert_eq!(endpoint(url).unwrap_err(), Error::Endpoint);
        }
        assert_eq!(model_name("qwen:cloud"), Err(Error::RemoteModel));
        assert_eq!(model_name("qwen:8b-cloud"), Err(Error::RemoteModel));
        assert!(model_name("qwen2.5-coder:0.5b").is_ok());
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
    };
    #[test]
    #[ignore = "requires loopback socket permission"]
    fn rejects_cloud_redirect_oversize_and_aborts_inflight_local_request() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for response in [
            "HTTP/1.1 302 Found\r\nLocation: https://example.org\r\nContent-Length: 0\r\n\r\n"
                .to_owned(),
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                MAX_REPLY + 1
            ),
            {
                let body = r#"{"remote_host":"https://ollama.com","remote_model":"qwen","details":{"format":"gguf"},"model_info":{}}"#;
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
            },
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let worker = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut buffer = [0; 8192];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(response.as_bytes());
            });
            let client = Client::new(&url).unwrap();
            let result = runtime.block_on(client.chat(
                "qwen:local",
                vec![json!({"role":"user","content":"hello"})],
                None,
            ));
            assert!(
                matches!(
                    result,
                    Err(Error::Status(302) | Error::Limit | Error::RemoteModel)
                ),
                "{result:?}"
            );
            worker.join().unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(AtomicUsize::new(0));
        let state = seen.clone();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut buffer = [0; 8192];
            let _ = stream.read(&mut buffer);
            state.store(1, Ordering::Release);
            // The future is aborted while waiting for headers; the peer must close promptly.
            while let Ok(count) = stream.read(&mut buffer) {
                if count == 0 {
                    state.store(2, Ordering::Release);
                    break;
                }
            }
        });
        runtime.block_on(async {
            let client = Client::new(&url).unwrap();
            let task = tokio::spawn(async move { client.models().await });
            let end = tokio::time::Instant::now() + Duration::from_secs(2);
            while seen.load(Ordering::Acquire) == 0 {
                assert!(tokio::time::Instant::now() < end);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        });
        worker.join().unwrap();
        assert_eq!(seen.load(Ordering::Acquire), 2);
    }
}
