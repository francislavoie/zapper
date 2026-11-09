//! A tiny HTTP server on 127.0.0.1, for tests that need a real socket.
//!
//! The same idea as the updater's (`updates::github` tests), with the
//! request headers kept, because the Tvheadend fakes answer differently
//! depending on the `Authorization` a request carries. One connection per
//! request, `Connection: close` on every answer, handled in turn on one
//! thread: the clients under test ask one thing at a time anyway, and it
//! keeps the order of [`Server::seen`] the order of the requests.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// The request target: path and query, as sent.
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The path without its query.
    pub fn route(&self) -> &str {
        self.path.split('?').next().unwrap_or(&self.path)
    }

    pub fn query(&self, key: &str) -> Option<&str> {
        self.path
            .split_once('?')?
            .1
            .split('&')
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
    }
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::new(200, body)
    }

    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.to_owned(), v.to_owned()));
        self
    }
}

pub struct Server {
    /// `http://127.0.0.1:<port>`
    pub base: String,
    pub seen: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

/// Serves `answer(request, base)` until the test process ends.
pub fn serve(answer: impl Fn(&Request, &str) -> Reply + Send + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let me = base.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            let mut parts = first.split_whitespace();
            let method = parts.next().unwrap_or("").to_owned();
            let path = parts.next().unwrap_or("").to_owned();
            let mut headers = Vec::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some((k, v)) = line.trim_end().split_once(':') {
                    if k.eq_ignore_ascii_case("content-length") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    headers.push((k.trim().to_owned(), v.trim().to_owned()));
                }
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            let req = Request {
                method,
                path,
                headers,
            };
            log.lock().unwrap().push(req.clone());
            let reply = answer(&req, &me);
            let mut out = format!(
                "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                reply.status,
                reply.body.len()
            );
            for (k, v) in &reply.headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            let _ = stream.write_all(out.as_bytes());
            let _ = stream.write_all(&reply.body);
        }
    });
    Server { base, seen }
}
