// The server name a TLS ClientHello asks for, read from the bytes a client sends first. Records
// are reassembled up to a limit; anything that is not a well-formed ClientHello is refused by the
// caller, never guessed at.

pub enum Hello {
    // more bytes are needed
    Incomplete,
    // the name the client asked for, if it asked for one
    Complete(Option<String>),
    Invalid,
}

const RECORD_HEADER: usize = 5;
const HANDSHAKE: u8 = 0x16;
const CLIENT_HELLO: u8 = 0x01;
pub const MAX_HELLO: usize = 64 * 1024;

pub fn parse(buf: &[u8]) -> Hello {
    // the handshake messages, gathered from as many records as it takes
    let mut handshake = Vec::new();
    let mut at = 0;
    loop {
        if buf.len() < at + RECORD_HEADER {
            return Hello::Incomplete;
        }
        if buf[at] != HANDSHAKE {
            return Hello::Invalid;
        }
        let len = u16::from_be_bytes([buf[at + 3], buf[at + 4]]) as usize;
        if buf.len() < at + RECORD_HEADER + len {
            return Hello::Incomplete;
        }
        handshake.extend_from_slice(&buf[at + RECORD_HEADER..at + RECORD_HEADER + len]);
        at += RECORD_HEADER + len;
        if handshake.len() >= 4 {
            if handshake[0] != CLIENT_HELLO {
                return Hello::Invalid;
            }
            let body = u32::from_be_bytes([0, handshake[1], handshake[2], handshake[3]]) as usize;
            if handshake.len() >= 4 + body {
                return match server_name(&handshake[4..4 + body]) {
                    Some(name) => Hello::Complete(name),
                    None => Hello::Invalid,
                };
            }
        }
        if at >= MAX_HELLO {
            return Hello::Invalid;
        }
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.buf.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(out)
    }
    fn u8(&mut self) -> Option<usize> {
        Some(self.take(1)?[0] as usize)
    }
    fn u16(&mut self) -> Option<usize> {
        let b = self.take(2)?;
        Some(u16::from_be_bytes([b[0], b[1]]) as usize)
    }
}

// None: malformed; Some(None): well formed without a server name
fn server_name(hello: &[u8]) -> Option<Option<String>> {
    let mut r = Reader { buf: hello, at: 0 };
    r.take(2 + 32)?; // version, random
    let n = r.u8()?;
    r.take(n)?; // session id
    let n = r.u16()?;
    r.take(n)?; // cipher suites
    let n = r.u8()?;
    r.take(n)?; // compression methods
    if r.at == hello.len() {
        return Some(None);
    }
    let n = r.u16()?;
    let mut ext = Reader {
        buf: r.take(n)?,
        at: 0,
    };
    while ext.at < ext.buf.len() {
        let kind = ext.u16()?;
        let n = ext.u16()?;
        let data = ext.take(n)?;
        if kind != 0 {
            continue;
        }
        let mut names = Reader { buf: data, at: 0 };
        let n = names.u16()?;
        let mut list = Reader {
            buf: names.take(n)?,
            at: 0,
        };
        while list.at < list.buf.len() {
            let name_type = list.u8()?;
            let n = list.u16()?;
            let name = list.take(n)?;
            if name_type == 0 {
                return Some(Some(String::from_utf8(name.to_vec()).ok()?));
            }
        }
        return Some(None);
    }
    Some(None)
}
