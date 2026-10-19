//! Talking to AI providers over HTTP. Providers send requests through a `Transport`, which is
//! `reqwest` in the app and `FakeApi` in tests, so no test calls a real provider.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
    /// How long to wait for the whole response, or for a streamed response's next chunk.
    pub timeout: Duration,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET",
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout: Duration::from_secs(120),
        }
    }

    pub fn post(url: impl Into<String>, body: Value) -> Self {
        Self {
            method: "POST",
            body: Some(body),
            ..Self::get(url)
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_owned(), value.into()));
        self
    }

    pub fn timeout(mut self, seconds: u64) -> Self {
        self.timeout = Duration::from_secs(seconds);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    Timeout,
    Connect,
}

/// A response whose body arrives in chunks.
pub struct Response {
    pub status: u16,
    body: Box<dyn Chunks>,
}

pub trait Chunks: Send {
    fn next(&mut self) -> BoxFuture<'_, Result<Option<Vec<u8>>, TransportError>>;
}

impl Response {
    pub fn new(status: u16, body: Box<dyn Chunks>) -> Self {
        Self { status, body }
    }

    /// The whole body.
    pub async fn bytes(mut self) -> Result<Vec<u8>, TransportError> {
        let mut all = Vec::new();
        while let Some(chunk) = self.body.next().await? {
            all.extend(chunk);
        }
        Ok(all)
    }

    /// The body line by line as it arrives, without line endings.
    pub fn lines(self) -> Lines {
        Lines {
            body: self.body,
            buffer: Vec::new(),
            done: false,
        }
    }
}

pub struct Lines {
    body: Box<dyn Chunks>,
    buffer: Vec<u8>,
    done: bool,
}

impl Lines {
    pub async fn next(&mut self) -> Result<Option<String>, TransportError> {
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }
            if self.done {
                if self.buffer.is_empty() {
                    return Ok(None);
                }
                let line = String::from_utf8_lossy(&self.buffer).into_owned();
                self.buffer.clear();
                return Ok(Some(line));
            }
            match self.body.next().await? {
                Some(chunk) => self.buffer.extend(chunk),
                None => self.done = true,
            }
        }
    }
}

pub trait Transport: Send + Sync {
    fn send(&self, request: Request) -> BoxFuture<'_, Result<Response, TransportError>>;
}

/// The real thing.
pub struct Reqwest {
    client: reqwest::Client,
}

impl Default for Reqwest {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("an HTTP client");
        Self { client }
    }
}

struct ReqwestBody {
    response: reqwest::Response,
    timeout: Duration,
}

impl Chunks for ReqwestBody {
    fn next(&mut self) -> BoxFuture<'_, Result<Option<Vec<u8>>, TransportError>> {
        Box::pin(async move {
            match tokio::time::timeout(self.timeout, self.response.chunk()).await {
                Err(_) => Err(TransportError::Timeout),
                Ok(Err(e)) if e.is_timeout() => Err(TransportError::Timeout),
                Ok(Err(_)) => Err(TransportError::Connect),
                Ok(Ok(chunk)) => Ok(chunk.map(|c| c.to_vec())),
            }
        })
    }
}

impl Transport for Reqwest {
    fn send(&self, request: Request) -> BoxFuture<'_, Result<Response, TransportError>> {
        Box::pin(async move {
            let method = reqwest::Method::from_bytes(request.method.as_bytes())
                .expect("a valid HTTP method");
            let mut builder = self.client.request(method, &request.url);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = &request.body {
                builder = builder.json(body);
            }
            let response = match tokio::time::timeout(request.timeout, builder.send()).await {
                Err(_) => return Err(TransportError::Timeout),
                Ok(Err(e)) if e.is_timeout() => return Err(TransportError::Timeout),
                Ok(Err(_)) => return Err(TransportError::Connect),
                Ok(Ok(response)) => response,
            };
            let status = response.status().as_u16();
            Ok(Response::new(
                status,
                Box::new(ReqwestBody {
                    response,
                    timeout: request.timeout,
                }),
            ))
        })
    }
}

/// Stands in for providers' HTTP APIs in tests: records requests, and answers each with the
/// next response queued for its method and URL (a 500 if there's none).
/// Responses queued by method and URL: each a status and a body.
type Queued = HashMap<(String, String), VecDeque<(u16, Vec<u8>)>>;

/// Works out a response from a request's JSON body: a status and a body.
type Responder = Arc<dyn Fn(&Value) -> (u16, Vec<u8>) + Send + Sync>;

#[derive(Default)]
pub struct FakeApi {
    responses: Mutex<Queued>,
    responders: Mutex<HashMap<(String, String), Responder>>,
    requests: Mutex<Vec<Request>>,
}

struct Whole(Option<Vec<u8>>);

impl Chunks for Whole {
    fn next(&mut self) -> BoxFuture<'_, Result<Option<Vec<u8>>, TransportError>> {
        Box::pin(async move { Ok(self.0.take()) })
    }
}

impl FakeApi {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Queues a JSON response.
    pub fn on(&self, method: &str, url: &str, status: u16, body: Value) {
        self.on_raw(method, url, status, body.to_string());
    }

    /// Queues a response with this body as it is, such as a stream of server-sent events.
    pub fn on_raw(&self, method: &str, url: &str, status: u16, body: impl Into<String>) {
        self.responses
            .lock()
            .unwrap()
            .entry((method.to_owned(), url.to_owned()))
            .or_default()
            .push_back((status, body.into().into_bytes()));
    }

    /// Queues a response with a binary body, such as a download.
    pub fn on_bytes(&self, method: &str, url: &str, status: u16, body: Vec<u8>) {
        self.responses
            .lock()
            .unwrap()
            .entry((method.to_owned(), url.to_owned()))
            .or_default()
            .push_back((status, body));
    }

    /// Answers every request to this method and URL that has nothing queued by calling
    /// `respond` with its body, like a model that reads each question.
    pub fn respond(
        &self,
        method: &str,
        url: &str,
        respond: impl Fn(&Value) -> (u16, Value) + Send + Sync + 'static,
    ) {
        self.respond_raw(method, url, move |body| {
            let (status, body) = respond(body);
            (status, body.to_string())
        });
    }

    /// As `respond`, with the response body as it is, such as a stream of server-sent events.
    pub fn respond_raw(
        &self,
        method: &str,
        url: &str,
        respond: impl Fn(&Value) -> (u16, String) + Send + Sync + 'static,
    ) {
        let respond = move |body: &Value| {
            let (status, body) = respond(body);
            (status, body.into_bytes())
        };
        self.responders
            .lock()
            .unwrap()
            .insert((method.to_owned(), url.to_owned()), Arc::new(respond));
    }

    /// The JSON bodies of the requests sent to this URL.
    pub fn bodies(&self, url: &str) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.url == url)
            .filter_map(|r| r.body.clone())
            .collect()
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    /// The body of the last request.
    pub fn last_body(&self) -> Value {
        self.requests
            .lock()
            .unwrap()
            .last()
            .and_then(|r| r.body.clone())
            .unwrap_or(Value::Null)
    }
}

impl Transport for FakeApi {
    fn send(&self, request: Request) -> BoxFuture<'_, Result<Response, TransportError>> {
        let key = (request.method.to_owned(), request.url.clone());
        let queued = self
            .responses
            .lock()
            .unwrap()
            .get_mut(&key)
            .and_then(VecDeque::pop_front);
        let responder = self.responders.lock().unwrap().get(&key).cloned();
        let (status, body) = match (queued, responder) {
            (Some(queued), _) => queued,
            (None, Some(respond)) => respond(request.body.as_ref().unwrap_or(&Value::Null)),
            (None, None) => {
                let message = format!("unexpected {} {}", key.0, key.1);
                (500, json!({ "error": message }).to_string().into_bytes())
            }
        };
        self.requests.lock().unwrap().push(request);
        Box::pin(async move { Ok(Response::new(status, Box::new(Whole(Some(body))))) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pieces(VecDeque<&'static str>);

    impl Chunks for Pieces {
        fn next(&mut self) -> BoxFuture<'_, Result<Option<Vec<u8>>, TransportError>> {
            Box::pin(async move { Ok(self.0.pop_front().map(|p| p.as_bytes().to_vec())) })
        }
    }

    #[tokio::test]
    async fn lines_join_across_chunks() {
        let body = Pieces(VecDeque::from(["data: a", "bc\r\n\nda", "ta: d"]));
        let mut lines = Response::new(200, Box::new(body)).lines();
        let mut seen = Vec::new();
        while let Some(line) = lines.next().await.unwrap() {
            seen.push(line);
        }
        assert_eq!(seen, ["data: abc", "", "data: d"]);
    }

    #[tokio::test]
    async fn the_fake_answers_in_order_then_with_an_error() {
        let api = FakeApi::new();
        api.on("GET", "https://x/models", 200, json!({"ok": 1}));
        let response = api.send(Request::get("https://x/models")).await.unwrap();
        assert_eq!(response.status, 200);
        let response = api.send(Request::get("https://x/models")).await.unwrap();
        assert_eq!(response.status, 500);
        assert_eq!(api.requests().len(), 2);
    }
}
