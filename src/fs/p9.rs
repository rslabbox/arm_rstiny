use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use log::{debug, info, warn};

use crate::drivers::virtio::p9::P9_DEVICE;
use crate::hal::Mutex;

const NO_FID: u32 = 0xFFFF_FFFF;
const NO_TAG: u16 = 0xFFFF;

const TVERSION: u8 = 100;
const RVERSION: u8 = 101;
const TATTACH: u8 = 104;
const RATTACH: u8 = 105;
const RERROR: u8 = 107;
const RLERROR: u8 = 7;
const TWALK: u8 = 110;
const RWALK: u8 = 111;
const TOPEN: u8 = 112;
const ROPEN: u8 = 113;
const TCREATE: u8 = 114;
const RCREATE: u8 = 115;
const TREAD: u8 = 116;
const RREAD: u8 = 117;
const TWRITE: u8 = 118;
const RWRITE: u8 = 119;
const TCLUNK: u8 = 120;
const RCLUNK: u8 = 121;
const TLOPEN: u8 = 12;
const RLOPEN: u8 = 13;
const TREADDIR: u8 = 40;
const RREADDIR: u8 = 41;

const OREAD: u8 = 0;
const OWRITE: u8 = 1;
const ORDWR: u8 = 2;

const DMDIR: u32 = 0x8000_0000;

const DEFAULT_MSIZE: u32 = 16384;

#[derive(Clone, Copy, Debug)]
struct Qid {
    type_: u8,
    _version: u32,
    _path: u64,
}

struct P9Session {
    msize: u32,
    next_tag: u16,
    next_fid: u32,
    root_fid: u32,
    mount_tag: String,
    version: String,
}

lazy_static! {
    static ref P9_SESSION: Mutex<Option<P9Session>> = Mutex::new(None);
}

pub fn init() {
    let mount_tag = match p9_mount_tag() {
        Some(tag) => tag,
        None => {
            warn!("virtio-9p device not available");
            return;
        }
    };

    let mut session = P9Session::new(mount_tag);
    match session.negotiate() {
        Ok(()) => {
            info!("virtio-9p mounted at /tools");
            *P9_SESSION.lock() = Some(session);
        }
        Err(err) => {
            warn!("virtio-9p session init failed: {}", err);
        }
    }
}

pub fn is_available() -> bool {
    P9_SESSION.lock().is_some()
}

pub fn list_dir(path: &str) -> Result<Vec<String>, String> {
    with_session(|session| session.list_dir(path))
}

pub fn change_dir(path: &str) -> Result<(), String> {
    with_session(|session| session.ensure_dir(path))
}

pub fn make_dir(path: &str) -> Result<(), String> {
    with_session(|session| session.create_dir(path))
}

fn with_session<T>(f: impl FnOnce(&mut P9Session) -> Result<T, String>) -> Result<T, String> {
    let mut guard = P9_SESSION.lock();
    let session = guard.as_mut().ok_or("virtio-9p not initialized")?;
    f(session)
}

fn p9_mount_tag() -> Option<String> {
    let guard = P9_DEVICE.lock();
    guard.as_ref().map(|dev| dev.mount_tag().to_string())
}

impl P9Session {
    fn new(mount_tag: String) -> Self {
        Self {
            msize: DEFAULT_MSIZE,
            next_tag: 1,
            next_fid: 2,
            root_fid: 1,
            mount_tag,
            version: String::from("unknown"),
        }
    }

    fn negotiate(&mut self) -> Result<(), String> {
        let mut last_version = String::from("unknown");
        for version in [
            "9p2000.L",
            "9p2000.u",
            "9p2000",
            "9P2000.L",
            "9P2000.u",
            "9P2000",
        ] {
            let resp = self.send_tversion(version)?;
            if resp.to_ascii_lowercase().starts_with("9p2000") {
                self.version = resp;
                self.send_tattach()?;
                return Ok(());
            }
            warn!(
                "RVERSION not accepted (req={}, resp={})",
                version,
                resp
            );
            last_version = resp;
        }
        Err(format!("unsupported 9p version: {}", last_version))
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<String>, String> {
        let (fid, is_dir) = self.walk_path(path)?;
        if !is_dir {
            self.clunk(fid)?;
            return Err(String::from("not a directory"));
        }

        self.open(fid, OREAD)?;

        let mut offset = 0u64;
        let mut names = Vec::new();
        loop {
            if self.version.to_ascii_lowercase().ends_with(".l") {
                let (chunk, next_offset) = self.readdir(fid, offset, self.msize - 64)?;
                if chunk.is_empty() {
                    break;
                }
                names.extend(chunk);
                match next_offset {
                    Some(next) if next > offset => offset = next,
                    _ => break,
                }
            } else {
                let data = self.read(fid, offset, self.msize - 64)?;
                if data.is_empty() {
                    break;
                }
                offset += data.len() as u64;
                parse_dir_entries(&data, &mut names)?;
            }
        }

        self.clunk(fid)?;
        Ok(names)
    }

    fn ensure_dir(&mut self, path: &str) -> Result<(), String> {
        let (fid, is_dir) = self.walk_path(path)?;
        self.clunk(fid)?;
        if is_dir {
            Ok(())
        } else {
            Err(String::from("not a directory"))
        }
    }

    fn create_dir(&mut self, path: &str) -> Result<(), String> {
        let (parent, name) = split_parent_name(path)?;
        let (fid, is_dir) = self.walk_path(parent)?;
        if !is_dir {
            self.clunk(fid)?;
            return Err(String::from("parent is not a directory"));
        }

        self.create(fid, name, OREAD, DMDIR | 0o755)?;
        self.clunk(fid)?;
        Ok(())
    }

    fn walk_path(&mut self, path: &str) -> Result<(u32, bool), String> {
        let fid = self.alloc_fid();
        let names = path_parts(path);
        let qids = self.walk(self.root_fid, fid, &names)?;
        let is_dir = qids.last().map(|q| q.type_ & 0x80 != 0).unwrap_or(true);
        Ok((fid, is_dir))
    }

    fn send_tversion(&mut self, version: &str) -> Result<String, String> {
        let tag = NO_TAG;
        let mut msg = Message::new(TVERSION, tag);
        msg.push_u32(self.msize);
        msg.push_str(version);
        let req = msg.finish();
        warn!("TVERSION raw (len={}): {}", req.len(), dump_hex(&req));
        let resp = self.send_recv(req, RVERSION, tag)?;
        warn!(
            "RVERSION raw (len={}): {}",
            resp.len(),
            dump_hex(&resp)
        );

        let mut offset = 0;
        let msize = read_u32(&resp, &mut offset)?;
        let version = match read_str(&resp, &mut offset) {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    "RVERSION parse error: {} (msize={}, remaining={})",
                    err,
                    msize,
                    dump_hex(&resp[offset..])
                );
                return Err(err);
            }
        };
        self.msize = msize.max(256);
        Ok(version)
    }

    fn send_tattach(&mut self) -> Result<(), String> {
        let tag = self.alloc_tag();
        let mut msg = Message::new(TATTACH, tag);
        msg.push_u32(self.root_fid);
        msg.push_u32(NO_FID);
        msg.push_str("root");
        msg.push_str(&self.mount_tag);
        if self.version.to_ascii_lowercase().ends_with(".l") {
            msg.push_u32(0);
        }
        let _ = self.send_recv(msg.finish(), RATTACH, tag)?;
        Ok(())
    }

    fn walk(&mut self, fid: u32, new_fid: u32, names: &[&str]) -> Result<Vec<Qid>, String> {
        let tag = self.alloc_tag();
        let mut msg = Message::new(TWALK, tag);
        msg.push_u32(fid);
        msg.push_u32(new_fid);
        msg.push_u16(names.len() as u16);
        for name in names {
            msg.push_str(name);
        }
        let resp = self.send_recv(msg.finish(), RWALK, tag)?;

        let mut offset = 0;
        let nwqid = read_u16(&resp, &mut offset)? as usize;
        if nwqid < names.len() {
            return Err(String::from("walk failed"));
        }

        let mut qids = Vec::with_capacity(nwqid);
        for _ in 0..nwqid {
            qids.push(read_qid(&resp, &mut offset)?);
        }
        Ok(qids)
    }

    fn open(&mut self, fid: u32, mode: u8) -> Result<(), String> {
        let tag = self.alloc_tag();
        if self.version.to_ascii_lowercase().ends_with(".l") {
            let mut msg = Message::new(TLOPEN, tag);
            msg.push_u32(fid);
            msg.push_u32(mode as u32);
            let _ = self.send_recv(msg.finish(), RLOPEN, tag)?;
        } else {
            let mut msg = Message::new(TOPEN, tag);
            msg.push_u32(fid);
            msg.push_u8(mode);
            let _ = self.send_recv(msg.finish(), ROPEN, tag)?;
        }
        Ok(())
    }

    fn create(&mut self, fid: u32, name: &str, mode: u8, perm: u32) -> Result<(), String> {
        let tag = self.alloc_tag();
        let mut msg = Message::new(TCREATE, tag);
        msg.push_u32(fid);
        msg.push_str(name);
        msg.push_u32(perm);
        msg.push_u8(mode);
        let _ = self.send_recv(msg.finish(), RCREATE, tag)?;
        Ok(())
    }

    fn read(&mut self, fid: u32, offset: u64, count: u32) -> Result<Vec<u8>, String> {
        let tag = self.alloc_tag();
        let mut msg = Message::new(TREAD, tag);
        msg.push_u32(fid);
        msg.push_u64(offset);
        msg.push_u32(count);
        let resp = self.send_recv(msg.finish(), RREAD, tag)?;

        let mut offset = 0;
        let data_len = read_u32(&resp, &mut offset)? as usize;
        if offset + data_len > resp.len() {
            return Err(String::from("short read response"));
        }
        Ok(resp[offset..offset + data_len].to_vec())
    }

    fn readdir(&mut self, fid: u32, offset: u64, count: u32) -> Result<(Vec<String>, Option<u64>), String> {
        let tag = self.alloc_tag();
        let mut msg = Message::new(TREADDIR, tag);
        msg.push_u32(fid);
        msg.push_u64(offset);
        msg.push_u32(count);
        let resp = self.send_recv(msg.finish(), RREADDIR, tag)?;

        let mut offset = 0;
        let data_len = read_u32(&resp, &mut offset)? as usize;
        if offset + data_len > resp.len() {
            return Err(String::from("short readdir response"));
        }
        parse_dir_entries_l(&resp[offset..offset + data_len])
    }

    #[allow(dead_code)]
    fn write(&mut self, fid: u32, offset: u64, data: &[u8]) -> Result<usize, String> {
        let tag = self.alloc_tag();
        let mut msg = Message::new(TWRITE, tag);
        msg.push_u32(fid);
        msg.push_u64(offset);
        msg.push_u32(data.len() as u32);
        msg.push_bytes(data);
        let resp = self.send_recv(msg.finish(), RWRITE, tag)?;

        let mut offset = 0;
        let wrote = read_u32(&resp, &mut offset)? as usize;
        Ok(wrote)
    }

    fn clunk(&mut self, fid: u32) -> Result<(), String> {
        let tag = self.alloc_tag();
        let mut msg = Message::new(TCLUNK, tag);
        msg.push_u32(fid);
        let _ = self.send_recv(msg.finish(), RCLUNK, tag)?;
        Ok(())
    }

    fn send_recv(&mut self, req: Vec<u8>, expect: u8, tag: u16) -> Result<Vec<u8>, String> {
        let mut resp = vec![0u8; self.msize as usize];
        let size = with_device(|dev| dev.request(&req, &mut resp))?;
        if size < 7 {
            return Err(String::from("short 9p response"));
        }
        let resp = &resp[..size];
        let resp_type = resp[4];
        let resp_tag = u16::from_le_bytes([resp[5], resp[6]]);
        if resp_type == RERROR {
            let mut offset = 7;
            let msg = read_str(resp, &mut offset).unwrap_or_else(|_| String::from("unknown"));
            return Err(msg);
        }
        if resp_type == RLERROR {
            let mut offset = 7;
            let errno = read_u32(resp, &mut offset).unwrap_or(0);
            return Err(format!("rlerror errno={}", errno));
        }
        if resp_type != expect {
            return Err(format!("unexpected response type: {}", resp_type));
        }
        if resp_tag != tag {
            return Err(String::from("tag mismatch"));
        }
        Ok(resp[7..].to_vec())
    }

    fn alloc_tag(&mut self) -> u16 {
        let mut tag = self.next_tag;
        self.next_tag = self.next_tag.wrapping_add(1);
        if tag == NO_TAG {
            tag = self.next_tag;
            self.next_tag = self.next_tag.wrapping_add(1);
        }
        tag
    }

    fn alloc_fid(&mut self) -> u32 {
        let fid = self.next_fid;
        self.next_fid = self.next_fid.wrapping_add(1);
        fid
    }
}

fn with_device<T>(f: impl FnOnce(&mut crate::drivers::virtio::p9::VirtIOP9) -> Result<T, virtio_drivers::Error>) -> Result<T, String> {
    let mut guard = P9_DEVICE.lock();
    let dev = guard.as_mut().ok_or("virtio-9p not initialized")?;
    f(dev).map_err(|e| format!("virtio-9p error: {:?}", e))
}

fn split_parent_name(path: &str) -> Result<(&str, &str), String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return Err(String::from("invalid path"));
    }
    let mut parts = trimmed.rsplitn(2, '/');
    let name = parts.next().unwrap_or("");
    let parent = parts.next().unwrap_or("");
    let parent = if parent.is_empty() { "/" } else { parent };
    if name.is_empty() {
        Err(String::from("invalid path"))
    } else {
        Ok((parent, name))
    }
}

fn path_parts(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect()
}

fn parse_dir_entries(data: &[u8], names: &mut Vec<String>) -> Result<(), String> {
    let mut offset = 0usize;
    while offset + 2 <= data.len() {
        let size = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + size > data.len() {
            break;
        }
        let entry = &data[offset..offset + size];
        offset += size;
        let name = parse_stat_name(entry)?;
        if name != "." && name != ".." {
            names.push(name);
        }
    }
    Ok(())
}

fn parse_dir_entries_l(data: &[u8]) -> Result<(Vec<String>, Option<u64>), String> {
    let mut offset = 0usize;
    let mut names = Vec::new();
    let mut last_offset = None;
    while offset < data.len() {
        let _qid = read_qid(data, &mut offset)?;
        let entry_offset = read_u64(data, &mut offset)?;
        let _entry_type = read_u8(data, &mut offset)?;
        let name = read_str(data, &mut offset)?;
        if name != "." && name != ".." {
            names.push(name);
        }
        last_offset = Some(entry_offset);
    }
    Ok((names, last_offset))
}

fn parse_stat_name(buf: &[u8]) -> Result<String, String> {
    let mut offset = 0usize;
    if buf.len() < 39 {
        return Err(String::from("stat too short"));
    }
    let _type = read_u16(buf, &mut offset)?;
    let _dev = read_u32(buf, &mut offset)?;
    let _qid = read_qid(buf, &mut offset)?;
    let _mode = read_u32(buf, &mut offset)?;
    let _atime = read_u32(buf, &mut offset)?;
    let _mtime = read_u32(buf, &mut offset)?;
    let _length = read_u64(buf, &mut offset)?;
    let name = read_str(buf, &mut offset)?;
    Ok(name)
}

struct Message {
    buf: Vec<u8>,
}

impl Message {
    fn new(msg_type: u8, tag: u16) -> Self {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(&[0, 0, 0, 0]);
        buf.push(msg_type);
        buf.extend_from_slice(&tag.to_le_bytes());
        Self { buf }
    }

    fn push_u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    fn push_u16(&mut self, value: u16) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    fn push_str(&mut self, value: &str) {
        let bytes = value.as_bytes();
        let len = bytes.len() as u16;
        self.push_u16(len);
        self.buf.extend_from_slice(bytes);
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    fn finish(mut self) -> Vec<u8> {
        let size = self.buf.len() as u32;
        self.buf[0..4].copy_from_slice(&size.to_le_bytes());
        self.buf
    }
}

fn read_u8(buf: &[u8], offset: &mut usize) -> Result<u8, String> {
    if *offset + 1 > buf.len() {
        return Err(String::from("short buffer"));
    }
    let value = buf[*offset];
    *offset += 1;
    Ok(value)
}

fn read_u16(buf: &[u8], offset: &mut usize) -> Result<u16, String> {
    if *offset + 2 > buf.len() {
        return Err(String::from("short buffer"));
    }
    let value = u16::from_le_bytes([buf[*offset], buf[*offset + 1]]);
    *offset += 2;
    Ok(value)
}

fn read_u32(buf: &[u8], offset: &mut usize) -> Result<u32, String> {
    if *offset + 4 > buf.len() {
        return Err(String::from("short buffer"));
    }
    let value = u32::from_le_bytes([
        buf[*offset],
        buf[*offset + 1],
        buf[*offset + 2],
        buf[*offset + 3],
    ]);
    *offset += 4;
    Ok(value)
}

fn read_u64(buf: &[u8], offset: &mut usize) -> Result<u64, String> {
    if *offset + 8 > buf.len() {
        return Err(String::from("short buffer"));
    }
    let value = u64::from_le_bytes([
        buf[*offset],
        buf[*offset + 1],
        buf[*offset + 2],
        buf[*offset + 3],
        buf[*offset + 4],
        buf[*offset + 5],
        buf[*offset + 6],
        buf[*offset + 7],
    ]);
    *offset += 8;
    Ok(value)
}

fn read_str(buf: &[u8], offset: &mut usize) -> Result<String, String> {
    let len = read_u16(buf, offset)? as usize;
    if *offset + len > buf.len() {
        return Err(String::from("short buffer"));
    }
    let value = core::str::from_utf8(&buf[*offset..*offset + len])
        .map_err(|_| String::from("invalid utf8"))?;
    *offset += len;
    Ok(value.to_string())
}

fn read_qid(buf: &[u8], offset: &mut usize) -> Result<Qid, String> {
    let type_ = read_u8(buf, offset)?;
    let version = read_u32(buf, offset)?;
    let path = read_u64(buf, offset)?;
    Ok(Qid {
        type_,
        _version: version,
        _path: path,
    })
}

fn dump_hex(buf: &[u8]) -> String {
    let mut out = String::new();
    for (idx, byte) in buf.iter().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{:02x}", byte));
    }
    out
}
