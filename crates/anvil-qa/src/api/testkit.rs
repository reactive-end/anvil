//! Test doubles for the scenario engine: a tiny HTTP server and a scripted database client.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use anyhow::{Result, anyhow};
use serde_json::Value;

use super::db::DbClient;

/// A local HTTP server that answers every request through a handler.
pub struct TestServer {
    pub url: String,
    log: Arc<Mutex<Vec<String>>>,
}

impl TestServer {
    /// The raw text of every request received so far.
    pub fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Answers the first request with the first `(status line, body)` pair, the next with the second,
/// and keeps repeating the last pair.
pub fn serve(responses: Vec<(&'static str, &'static str)>) -> TestServer {
    let counter = Arc::new(Mutex::new(0usize));
    serve_with(move |_| {
        let mut seen = counter.lock().unwrap_or_else(PoisonError::into_inner);
        let (status, body) = responses[(*seen).min(responses.len() - 1)];
        *seen += 1;
        (status.to_owned(), body.to_owned())
    })
}

/// Answers each request with the `(status line, JSON body)` the handler returns for its raw text.
pub fn serve_with(handler: impl Fn(&str) -> (String, String) + Send + 'static) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port());
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&log);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let request = read_request(&stream);
            seen.lock().unwrap_or_else(PoisonError::into_inner).push(request.clone());
            let (status, body) = handler(&request);
            reply(stream, &status, &body);
        }
    });
    TestServer { url, log }
}

fn read_request(mut stream: &TcpStream) -> String {
    let mut data = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).unwrap_or(0);
        data.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&data);
        let Some(head_end) = text.find("\r\n\r\n") else {
            if read == 0 {
                return text.into_owned();
            }
            continue;
        };
        let length = text[..head_end]
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase().strip_prefix("content-length:")?.trim().parse().ok()
            })
            .unwrap_or(0usize);
        if data.len() >= head_end + 4 + length || read == 0 {
            return text.into_owned();
        }
    }
}

fn reply(mut stream: TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// A database client that returns scripted rows and remembers what it was asked.
#[derive(Default)]
pub struct FakeDb {
    replies: Mutex<VecDeque<std::result::Result<Vec<Value>, String>>>,
    calls: Mutex<Vec<(String, Value)>>,
}

impl FakeDb {
    pub fn new() -> Self {
        Self::default()
    }

    /// The next call returns `rows`.
    pub fn then_rows(self, rows: Vec<Value>) -> Self {
        self.replies.lock().unwrap_or_else(PoisonError::into_inner).push_back(Ok(rows));
        self
    }

    /// The next call fails with `message`.
    pub fn then_error(self, message: &str) -> Self {
        let failure = Err(message.to_owned());
        self.replies.lock().unwrap_or_else(PoisonError::into_inner).push_back(failure);
        self
    }

    /// The `(tool, params)` of every call so far.
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl DbClient for FakeDb {
    fn invoke(&self, tool: &str, params: &Value) -> Result<Vec<Value>> {
        let call = (tool.to_owned(), params.clone());
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(call);
        let next = self.replies.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
        next.unwrap_or_else(|| Ok(Vec::new())).map_err(|message| anyhow!("toolbox: {message}"))
    }
}
