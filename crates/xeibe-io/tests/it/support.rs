//! A scripted HTTP/1.1 server on a local port: each connection gets the next
//! reply of the script and is closed, so every request is a new connection.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

pub enum Reply {
    /// A complete response.
    Full {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    },
    /// A `200` that announces `declared` bytes, sends `sent` and hangs up.
    Broken { declared: usize, sent: Vec<u8> },
}

impl Reply {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Reply::Full {
            status: 200,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn status(status: u16) -> Self {
        Reply::Full {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn retry_after(status: u16, seconds: u64) -> Self {
        Reply::Full {
            status,
            headers: vec![("Retry-After", seconds.to_string())],
            body: Vec::new(),
        }
    }
}

pub struct Server {
    pub base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Server {
    /// Serves `replies` in order, then stops listening.
    pub fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding a local port");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        std::thread::spawn(move || {
            for reply in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let head = read_head(&mut stream);
                log.lock().unwrap().push(head);
                let _ = respond(&mut stream, reply);
            }
        });
        Self { base, requests }
    }

    pub fn url(&self, path: &str) -> url::Url {
        url::Url::parse(&format!("{}{path}", self.base)).unwrap()
    }

    /// Request heads received so far (request line and headers).
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

fn respond(stream: &mut TcpStream, reply: Reply) -> std::io::Result<()> {
    match reply {
        Reply::Full {
            status,
            headers,
            body,
        } => {
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            )?;
            for (name, value) in headers {
                write!(stream, "{name}: {value}\r\n")?;
            }
            stream.write_all(b"\r\n")?;
            // In pieces, as a real server would.
            for piece in body.chunks(16 << 10) {
                stream.write_all(piece)?;
            }
        }
        Reply::Broken { declared, sent } => {
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n"
            )?;
            stream.write_all(&sent)?;
        }
    }
    stream.flush()?;
    stream.shutdown(std::net::Shutdown::Both)
}

/// `n` bytes of a GML-like document, distinct enough to catch reordering.
pub fn body(n: usize) -> Vec<u8> {
    let mut out = b"<gml:FeatureCollection>".to_vec();
    let mut i = 0;
    while out.len() < n {
        out.extend_from_slice(format!("<f>{i}</f>").as_bytes());
        i += 1;
    }
    out.truncate(n);
    out
}
