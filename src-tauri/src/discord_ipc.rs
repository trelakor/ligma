//! Connection to the Discord client's IPC socket (a named pipe on
//! Windows). `discord-rich-presence` only writes to it, but invites need
//! Discord's events too (`ACTIVITY_JOIN`, `ACTIVITY_JOIN_REQUEST`), read
//! here without blocking so one thread can both send and receive
//!
//! Each frame is an opcode and a length, both little-endian u32, then that
//! many bytes of JSON

use std::io::{self, Read, Write};

use serde_json::{json, Value};

const HANDSHAKE: u32 = 0;
pub const FRAME: u32 = 1;
const CLOSE: u32 = 2;
const PING: u32 = 3;
const PONG: u32 = 4;

#[cfg(windows)]
type Socket = std::fs::File;
#[cfg(unix)]
type Socket = std::os::unix::net::UnixStream;

pub struct Ipc {
    socket: Socket,
}

#[cfg(windows)]
fn open(i: u32) -> io::Result<Socket> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!(r"\\?\pipe\discord-ipc-{i}"))
}

#[cfg(unix)]
fn open(i: u32) -> io::Result<Socket> {
    let dir = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .find_map(std::env::var_os)
        .unwrap_or_else(|| "/tmp".into());
    Socket::connect(std::path::Path::new(&dir).join(format!("discord-ipc-{i}")))
}

fn nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed).to_string()
}

impl Ipc {
    /// Connects as application `client_id` and waits for Discord's READY
    pub fn connect(client_id: &str) -> io::Result<Self> {
        let socket = (0..10)
            .find_map(|i| open(i).ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Discord isn't running"))?;
        let mut ipc = Ipc { socket };
        ipc.send(HANDSHAKE, &json!({ "v": 1, "client_id": client_id }))?;
        match ipc.read_frame()? {
            (FRAME, ready) if ready["evt"] == "READY" => Ok(ipc),
            _ => Err(io::Error::new(io::ErrorKind::ConnectionRefused, "handshake rejected")),
        }
    }

    fn send(&mut self, op: u32, payload: &Value) -> io::Result<()> {
        let body = payload.to_string();
        let mut frame = Vec::with_capacity(8 + body.len());
        frame.extend_from_slice(&op.to_le_bytes());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(body.as_bytes());
        self.socket.write_all(&frame)
    }

    /// Sends RPC command `cmd`. Its response arrives later from `poll`
    pub fn command(&mut self, cmd: &str, args: Value) -> io::Result<()> {
        self.send(FRAME, &json!({ "cmd": cmd, "args": args, "nonce": nonce() }))
    }

    /// Asks Discord to send event `evt`
    pub fn subscribe(&mut self, evt: &str) -> io::Result<()> {
        self.send(FRAME, &json!({ "cmd": "SUBSCRIBE", "evt": evt, "nonce": nonce() }))
    }

    /// Next message from Discord, if one has arrived. Answers pings itself
    pub fn poll(&mut self) -> io::Result<Option<Value>> {
        while let Some(header) = self.try_header()? {
            match self.read_body(header)? {
                (FRAME, message) => return Ok(Some(message)),
                (PING, data) => self.send(PONG, &data)?,
                (CLOSE, _) => return Err(io::ErrorKind::ConnectionAborted.into()),
                _ => {}
            }
        }
        Ok(None)
    }

    fn read_frame(&mut self) -> io::Result<(u32, Value)> {
        let mut header = [0; 8];
        self.socket.read_exact(&mut header)?;
        self.read_body(header)
    }

    fn read_body(&mut self, header: [u8; 8]) -> io::Result<(u32, Value)> {
        let op = u32::from_le_bytes(header[..4].try_into().unwrap());
        let len = u32::from_le_bytes(header[4..].try_into().unwrap());
        let mut body = vec![0; len as usize];
        self.socket.read_exact(&mut body)?;
        Ok((op, serde_json::from_slice(&body).unwrap_or(Value::Null)))
    }

    /// The next frame's header, if one has arrived. Discord writes a frame
    /// in one go, so its body follows right away
    #[cfg(windows)]
    fn try_header(&mut self) -> io::Result<Option<[u8; 8]>> {
        use std::os::windows::io::AsRawHandle;

        #[link(name = "kernel32")]
        extern "system" {
            fn PeekNamedPipe(
                pipe: *mut std::ffi::c_void,
                buffer: *mut std::ffi::c_void,
                size: u32,
                read: *mut u32,
                available: *mut u32,
                left: *mut u32,
            ) -> i32;
        }
        let mut available = 0;
        let ok = unsafe {
            PeekNamedPipe(
                self.socket.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if available < 8 {
            return Ok(None);
        }
        let mut header = [0; 8];
        self.socket.read_exact(&mut header)?;
        Ok(Some(header))
    }

    #[cfg(unix)]
    fn try_header(&mut self) -> io::Result<Option<[u8; 8]>> {
        let mut header = [0; 8];
        self.socket.set_nonblocking(true)?;
        let read = self.socket.read(&mut header);
        self.socket.set_nonblocking(false)?;
        let read = match read {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e),
        };
        self.socket.read_exact(&mut header[read..])?;
        Ok(Some(header))
    }
}
