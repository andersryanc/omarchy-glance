//! Touch Bar hardware IPC v1 (see t1bridge-interfaces.md): the socket, the
//! wire format and the shared frame buffers.

use std::collections::HashMap;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use cairo::{Format, ImageSurface};

pub const SOCK_PATH: &str = "/run/t1bridge/touchbar.sock";
const MAGIC: &[u8; 4] = b"T1HW";
const MAJOR: u16 = 1;
pub const HDR_SIZE: usize = 16;

pub const HELLO: u16 = 0x0001;
pub const REGISTER_BUFFER: u16 = 0x0002;
pub const SUBMIT_FRAME: u16 = 0x0003;
pub const TAP_KEYS: u16 = 0x0004;
pub const HELLO_ACK: u16 = 0x8001;
pub const ACK: u16 = 0x8002;
pub const ERROR: u16 = 0x8003;
pub const FRAME_RELEASED: u16 = 0x9001;
pub const INPUT_FRAME: u16 = 0x9002;

pub const FEAT_MEMFD: u64 = 0x01;
pub const FEAT_INPUT: u64 = 0x02;
pub const FEAT_KEYS: u64 = 0x04;

pub fn error_name(code: u32) -> &'static str {
    match code {
        1 => "unsupported message",
        2 => "unsupported feature",
        3 => "resource limit",
        4 => "invalid buffer",
        5 => "unknown buffer",
        6 => "buffer busy",
        7 => "action denied",
        8 => "device unavailable",
        9 => "I/O failure",
        10 => "internal failure",
        _ => "?",
    }
}

/// Little-endian payload builder.
#[derive(Default)]
pub struct Packer(pub Vec<u8>);

impl Packer {
    pub fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    pub fn u16(mut self, v: u16) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(mut self, v: u64) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn pad(mut self, n: usize) -> Self {
        self.0.extend(std::iter::repeat_n(0, n));
        self
    }
}

pub fn le_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

pub fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

pub fn le_u64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

pub struct Conn {
    fd: OwnedFd,
    next_req: u32,
    pub pending: HashMap<u32, String>, // req id -> description
}

impl Conn {
    pub fn connect(path: &str) -> io::Result<Conn> {
        unsafe {
            let raw = libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0);
            if raw < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = OwnedFd::from_raw_fd(raw);
            let mut addr: libc::sockaddr_un = std::mem::zeroed();
            addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
            for (d, s) in addr.sun_path.iter_mut().zip(path.bytes()) {
                *d = s as libc::c_char;
            }
            let len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
            if libc::connect(raw, &addr as *const _ as *const libc::sockaddr, len) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Conn { fd, next_req: 1, pending: HashMap::new() })
        }
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Send one message, optionally passing a file descriptor with it.
    pub fn send(&mut self, mtype: u16, payload: &[u8], pass_fd: Option<RawFd>, what: &str) -> io::Result<u32> {
        let req = self.next_req;
        self.next_req = self.next_req % u32::MAX + 1;
        let mut pkt = Packer(MAGIC.to_vec()).u16(MAJOR).u16(mtype).u32(payload.len() as u32).u32(req).0;
        pkt.extend_from_slice(payload);
        unsafe {
            let mut iov = libc::iovec { iov_base: pkt.as_mut_ptr() as *mut _, iov_len: pkt.len() };
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            let space = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize;
            let mut control = vec![0u8; space];
            if let Some(fd) = pass_fd {
                msg.msg_control = control.as_mut_ptr() as *mut _;
                msg.msg_controllen = space;
                let cmsg = libc::CMSG_FIRSTHDR(&msg);
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as usize;
                std::ptr::write_unaligned(libc::CMSG_DATA(cmsg) as *mut RawFd, fd);
            }
            if libc::sendmsg(self.fd.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        self.pending.insert(req, if what.is_empty() { format!("{mtype:#06x}") } else { what.to_string() });
        Ok(req)
    }

    /// Receive one message: (type, req id, payload).
    pub fn recv(&mut self) -> io::Result<(u16, u32, Vec<u8>)> {
        let mut buf = vec![0u8; 65536];
        let n = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len(), 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let n = n as usize;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "service closed the connection"));
        }
        if n < HDR_SIZE || &buf[..4] != MAGIC || le_u16(&buf, 4) != MAJOR
            || n != HDR_SIZE + le_u32(&buf, 8) as usize
        {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "malformed packet"));
        }
        Ok((le_u16(&buf, 6), le_u32(&buf, 12), buf[HDR_SIZE..n].to_vec()))
    }
}

/// A sealed memfd shared with the service, mapped here and wrapped in a cairo
/// surface. Cairo's RGB24 is the same little-endian XRGB8888 the service wants.
pub struct Buffer {
    pub fd: OwnedFd,
    ptr: *mut u8,
    pub len: usize,
    pub surface: ImageSurface,
}

impl Buffer {
    pub fn new(id: u32, w: i32, h: i32, stride: i32) -> io::Result<Buffer> {
        let len = (stride * h) as usize;
        unsafe {
            let name = CString::new(format!("touchbar-{id}")).unwrap();
            let raw = libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING);
            if raw < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = OwnedFd::from_raw_fd(raw);
            if libc::ftruncate(raw, len as libc::off_t) < 0
                || libc::fcntl(raw, libc::F_ADD_SEALS, libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL) < 0
            {
                return Err(io::Error::last_os_error());
            }
            let ptr = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE,
                                 libc::MAP_SHARED, raw, 0);
            if ptr == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            let ptr = ptr as *mut u8;
            let surface = ImageSurface::create_for_data_unsafe(ptr, Format::Rgb24, w, h, stride)
                .map_err(|e| io::Error::other(e.to_string()))?;
            Ok(Buffer { fd, ptr, len, surface })
        }
    }

    /// Copy a complete frame from another buffer into this one.
    pub fn copy_from(&self, other: &Buffer) {
        self.surface.flush();
        unsafe { std::ptr::copy_nonoverlapping(other.ptr, self.ptr, self.len.min(other.len)) };
        self.surface.mark_dirty();
    }
}
