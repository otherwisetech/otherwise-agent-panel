//! A tiny blocking HTTP/1.1 client, enough for Ollama's JSON API: plain `http://` only (a local
//! or LAN server), `Content-Length` and chunked bodies, one connection per request. No new
//! dependency for the host apps.
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Where requests go: `http://host:port/base`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    pub host: String,
    pub port: u16,
    /// Path prefix without the trailing `/` (usually empty).
    pub path: String,
}

impl Base {
    pub fn parse(url: &str) -> Result<Base, String> {
        let url = url.trim();
        let rest = match url.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
            Some((scheme, _)) => return Err(format!("{scheme}:// is not supported, use http://")),
            None => url,
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
            None => (rest, ""),
        };
        if authority.is_empty() || authority.contains('@') {
            return Err(format!("bad address {url}"));
        }
        let (host, port) = if let Some(h) = authority.strip_prefix('[') {
            let (h, after) = h.split_once(']').ok_or_else(|| format!("bad address {url}"))?;
            (h.to_string(), after.strip_prefix(':').map(str::parse).transpose().map_err(|_| format!("bad port in {url}"))?)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p.parse().map_err(|_| format!("bad port in {url}"))?)),
                None => (authority.to_string(), None),
            }
        };
        if host.is_empty() {
            return Err(format!("bad address {url}"));
        }
        Ok(Base { host, port: port.unwrap_or(80), path: path.to_string() })
    }

    /// `localhost`, `127.x`, `::1`: the data stays on this computer.
    pub fn is_local(&self) -> bool {
        let h = self.host.to_ascii_lowercase();
        h == "localhost" || h.ends_with(".localhost") || h.starts_with("127.") || h == "::1" || h == "0.0.0.0"
    }

    fn host_header(&self) -> String {
        if self.host.contains(':') { format!("[{}]:{}", self.host, self.port) } else { format!("{}:{}", self.host, self.port) }
    }
}

/// A response whose body is read as it arrives. `stream` is a handle on the same socket, for
/// closing it from another thread (stopping a run).
pub struct Response {
    pub status: u16,
    pub body: Box<dyn BufRead + Send>,
    pub stream: TcpStream,
}

impl Response {
    pub fn text(mut self) -> io::Result<String> {
        let mut s = String::new();
        self.body.read_to_string(&mut s)?;
        Ok(s)
    }
}

pub fn request(base: &Base, method: &str, path: &str, body: Option<&[u8]>, connect: Duration, read: Duration) -> io::Result<Response> {
    let addrs: Vec<_> = (base.host.as_str(), base.port).to_socket_addrs()?.collect();
    let mut last = io::Error::new(io::ErrorKind::NotFound, format!("{} not found", base.host));
    let mut stream = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, connect) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = e,
        }
    }
    let mut stream = stream.ok_or(last)?;
    stream.set_read_timeout(Some(read))?;
    stream.set_write_timeout(Some(read))?;
    let _ = stream.set_nodelay(true);
    let body = body.unwrap_or(&[]);
    let head = format!(
        "{method} {}{path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: otherwise-agent-panel/{}\r\nAccept: application/json, application/x-ndjson\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        base.path,
        base.host_header(),
        crate::VERSION,
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    let handle = stream.try_clone()?;
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let status: u16 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not an HTTP response"))?;
    let (mut chunked, mut length) = (false, None);
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            break;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            } else if k == "content-length" {
                length = v.parse::<u64>().ok();
            }
        }
    }
    let body: Box<dyn BufRead + Send> = if chunked {
        Box::new(BufReader::new(Chunked { inner: r, left: 0, done: false }))
    } else if let Some(n) = length {
        Box::new(BufReader::new(r.take(n)))
    } else {
        Box::new(r)
    };
    Ok(Response { status, body, stream: handle })
}

/// `Transfer-Encoding: chunked` → the plain body.
struct Chunked<R> {
    inner: R,
    left: u64,
    done: bool,
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let mut line = String::new();
            // the CRLF that ends the previous chunk, then the size line
            while line.trim().is_empty() {
                line.clear();
                if self.inner.read_line(&mut line)? == 0 {
                    self.done = true;
                    return Ok(0);
                }
            }
            let size = line.trim().split(';').next().unwrap_or("");
            self.left = u64::from_str_radix(size, 16).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad chunk size"))?;
            if self.left == 0 {
                self.done = true;
                return Ok(0);
            }
        }
        let want = buf.len().min(self.left as usize);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            self.done = true;
        }
        self.left -= n as u64;
        Ok(n)
    }
}
