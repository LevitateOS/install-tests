//! Loopback-only, read-only HTTP origin for a signed integration fixture.
//! Build this helper explicitly for the guest architecture and put it on RECAB_P13.
use std::{
    env, fs,
    io::{self, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    time::Duration,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    let value = |name: &str| args.windows(2).find(|p| p[0] == name).map(|p| p[1].clone());
    let root = fs::canonicalize(value("--root").ok_or("--root required")?)?;
    let port = value("--port").ok_or("--port required")?.parse::<u16>()?;
    let ready = PathBuf::from(value("--ready").ok_or("--ready required")?);
    let once = value("--interrupt-once").map(PathBuf::from);
    let interrupt_path = value("--interrupt-path");
    if once.is_some() != interrupt_path.is_some() {
        return Err("--interrupt-once requires --interrupt-path".into());
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
    fs::write(&ready, format!("{}\n", listener.local_addr()?))?;
    for stream in listener.incoming() {
        let mut stream = stream?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        if let Err(e) = serve(
            &mut stream,
            &root,
            once.as_deref(),
            interrupt_path.as_deref(),
        ) {
            eprintln!("origin request: {e}");
        }
    }
    Ok(())
}
fn serve(
    stream: &mut TcpStream,
    root: &Path,
    once: Option<&Path>,
    interrupt_path: Option<&str>,
) -> io::Result<()> {
    let mut input = Vec::new();
    let mut byte = [0];
    while input.len() < 8192 {
        if stream.read(&mut byte)? == 0 {
            return Ok(());
        }
        input.push(byte[0]);
        if input.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let line = std::str::from_utf8(&input)
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("");
    let mut words = line.split_whitespace();
    let method = words.next().unwrap_or("");
    let path = words.next().unwrap_or("");
    if method != "GET" || !input.ends_with(b"\r\n\r\n") {
        return response(stream, "400 Bad Request");
    }
    // Release paths are validated UTF-8 relative paths. Decode only ordinary
    // percent bytes, then revalidate every component and reject symlinks.
    let mut bytes = Vec::new();
    let raw = path.as_bytes();
    let mut i = 1;
    if !path.starts_with('/') || path.contains(['?', '#']) {
        return response(stream, "400 Bad Request");
    }
    while i < raw.len() {
        if raw[i] == b'%' {
            if i + 2 >= raw.len() {
                return response(stream, "400 Bad Request");
            }
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).unwrap_or("");
            let Ok(b) = u8::from_str_radix(hex, 16) else {
                return response(stream, "400 Bad Request");
            };
            bytes.push(b);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let Ok(name) = std::str::from_utf8(&bytes) else {
        return response(stream, "400 Bad Request");
    };
    if name.is_empty()
        || name.contains(['\0', '\\'])
        || name
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return response(stream, "400 Bad Request");
    }
    let mut file = root.to_owned();
    for c in Path::new(name).components() {
        if !matches!(c, Component::Normal(_)) {
            return response(stream, "400 Bad Request");
        }
        file.push(c);
        let Ok(m) = fs::symlink_metadata(&file) else {
            return response(stream, "404 Not Found");
        };
        if m.file_type().is_symlink() {
            return response(stream, "404 Not Found");
        }
    }
    if !fs::symlink_metadata(&file)?.is_file() {
        return response(stream, "404 Not Found");
    }
    let mut source = fs::File::open(&file)?;
    let metadata = source.metadata()?;
    if !metadata.is_file() {
        return response(stream, "404 Not Found");
    }
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        metadata.len()
    )?;
    let interrupt = Some(name) == interrupt_path
        && once.is_some_and(|p| {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(p)
                .is_ok()
        });
    if interrupt {
        io::copy(&mut source.take(metadata.len() / 2), stream)?;
    } else {
        io::copy(&mut source, stream)?;
    }
    stream.flush()
}
fn response(s: &mut TcpStream, status: &str) -> io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}
