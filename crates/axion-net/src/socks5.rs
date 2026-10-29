//! A minimal SOCKS5 (RFC 1928) client and server with username/password login (RFC 1929).
//!
//! Only `CONNECT` is spoken. Targets are sent as domain names whenever they are names, so
//! the proxy resolves them: this computer never does.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use crate::conn::Address;
use crate::secret::Credentials;

const VERSION: u8 = 5;
const AUTH_VERSION: u8 = 1;
const METHOD_NONE: u8 = 0x00;
const METHOD_PASSWORD: u8 = 0x02;
const METHOD_NO_ACCEPTABLE: u8 = 0xFF;
const CMD_CONNECT: u8 = 1;
const ATYP_IPV4: u8 = 1;
const ATYP_DOMAIN: u8 = 3;
const ATYP_IPV6: u8 = 4;

/// Reply codes of RFC 1928.
pub mod reply {
    pub const SUCCEEDED: u8 = 0;
    pub const GENERAL_FAILURE: u8 = 1;
    pub const NOT_ALLOWED: u8 = 2;
    pub const NETWORK_UNREACHABLE: u8 = 3;
    pub const HOST_UNREACHABLE: u8 = 4;
    pub const CONNECTION_REFUSED: u8 = 5;
    pub const COMMAND_NOT_SUPPORTED: u8 = 7;
    pub const ADDRESS_NOT_SUPPORTED: u8 = 8;
}

#[derive(Debug, thiserror::Error)]
pub enum SocksError {
    #[error("the proxy did not speak SOCKS5")]
    NotSocks5,
    #[error("the proxy accepts none of our login methods")]
    NoAcceptableMethod,
    #[error("the proxy refused the user name or password")]
    AuthFailed,
    #[error("the proxy refused the connection (SOCKS reply {0})")]
    Refused(u8),
    #[error("the address cannot be sent: {0}")]
    BadAddress(&'static str),
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl SocksError {
    /// The reply code to give a client when this error ended a request.
    pub fn reply_code(&self) -> u8 {
        match self {
            SocksError::Refused(c) => *c,
            SocksError::Io(e) => match e.kind() {
                io::ErrorKind::ConnectionRefused => reply::CONNECTION_REFUSED,
                io::ErrorKind::TimedOut => reply::HOST_UNREACHABLE,
                io::ErrorKind::PermissionDenied => reply::NOT_ALLOWED,
                _ => reply::GENERAL_FAILURE,
            },
            _ => reply::GENERAL_FAILURE,
        }
    }
}

fn write_address(out: &mut Vec<u8>, target: &Address) -> Result<(), SocksError> {
    match target {
        Address::Domain(host, port) => {
            let bytes = host.as_bytes();
            if bytes.is_empty() || bytes.len() > 255 {
                return Err(SocksError::BadAddress("a host name is 1 to 255 bytes"));
            }
            out.extend([ATYP_DOMAIN, bytes.len() as u8]);
            out.extend(bytes);
            out.extend(port.to_be_bytes());
        }
        Address::Ip(SocketAddr::V4(a)) => {
            out.push(ATYP_IPV4);
            out.extend(a.ip().octets());
            out.extend(a.port().to_be_bytes());
        }
        Address::Ip(SocketAddr::V6(a)) => {
            out.push(ATYP_IPV6);
            out.extend(a.ip().octets());
            out.extend(a.port().to_be_bytes());
        }
    }
    Ok(())
}

/// Reads an address (after its type byte) and port.
fn read_address<R: Read>(r: &mut R, atyp: u8) -> Result<Address, SocksError> {
    match atyp {
        ATYP_IPV4 => {
            let mut b = [0u8; 6];
            r.read_exact(&mut b)?;
            let ip = Ipv4Addr::new(b[0], b[1], b[2], b[3]);
            Ok(Address::Ip(SocketAddr::V4(SocketAddrV4::new(ip, u16::from_be_bytes([b[4], b[5]])))))
        }
        ATYP_IPV6 => {
            let mut b = [0u8; 18];
            r.read_exact(&mut b)?;
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&b[..16]);
            Ok(Address::Ip(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(octets),
                u16::from_be_bytes([b[16], b[17]]),
                0,
                0,
            ))))
        }
        ATYP_DOMAIN => {
            let mut len = [0u8; 1];
            r.read_exact(&mut len)?;
            let mut host = vec![0u8; len[0] as usize + 2];
            r.read_exact(&mut host)?;
            let port = u16::from_be_bytes([host[host.len() - 2], host[host.len() - 1]]);
            host.truncate(host.len() - 2);
            let host =
                String::from_utf8(host).map_err(|_| SocksError::BadAddress("host name is not text"))?;
            Ok(Address::Domain(host, port))
        }
        _ => Err(SocksError::BadAddress("unknown address type")),
    }
}

/// Client side, step one: greeting and login (when the proxy asks for it).
pub fn client_negotiate<S: Read + Write>(
    stream: &mut S,
    auth: Option<&Credentials>,
) -> Result<(), SocksError> {
    let methods: &[u8] = if auth.is_some() { &[METHOD_NONE, METHOD_PASSWORD] } else { &[METHOD_NONE] };
    let mut hello = vec![VERSION, methods.len() as u8];
    hello.extend(methods);
    stream.write_all(&hello)?;
    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice)?;
    if choice[0] != VERSION {
        return Err(SocksError::NotSocks5);
    }
    match choice[1] {
        METHOD_NONE => {}
        METHOD_PASSWORD => {
            let creds = auth.ok_or(SocksError::NoAcceptableMethod)?;
            let (user, pass) = (creds.username.as_bytes(), creds.password.expose().as_bytes());
            if user.len() > 255 || pass.len() > 255 {
                return Err(SocksError::BadAddress("user name or password longer than 255 bytes"));
            }
            let mut msg = vec![AUTH_VERSION, user.len() as u8];
            msg.extend(user);
            msg.push(pass.len() as u8);
            msg.extend(pass);
            stream.write_all(&msg)?;
            let mut status = [0u8; 2];
            stream.read_exact(&mut status)?;
            if status[1] != 0 {
                return Err(SocksError::AuthFailed);
            }
        }
        _ => return Err(SocksError::NoAcceptableMethod),
    }
    Ok(())
}

/// Client side: negotiates (with a login when given) and asks the proxy to connect to
/// `target`. On success the stream is the connection to the target.
pub fn client_connect<S: Read + Write>(
    stream: &mut S,
    target: &Address,
    auth: Option<&Credentials>,
) -> Result<(), SocksError> {
    client_negotiate(stream, auth)?;
    let mut request = vec![VERSION, CMD_CONNECT, 0];
    write_address(&mut request, target)?;
    stream.write_all(&request)?;
    let mut head = [0u8; 4];
    stream.read_exact(&mut head)?;
    if head[0] != VERSION {
        return Err(SocksError::NotSocks5);
    }
    if head[1] != reply::SUCCEEDED {
        return Err(SocksError::Refused(head[1]));
    }
    // The bound address is of no use to us; read and drop it.
    read_address(stream, head[3])?;
    Ok(())
}

/// Server side, step one: greeting, login and request. Returns the address the client wants;
/// answer with [`server_reply`] once the connection is made (or has failed).
///
/// `required`: when set, the client must log in with exactly these credentials.
pub fn server_handshake<S: Read + Write>(
    stream: &mut S,
    required: Option<&Credentials>,
) -> Result<Address, SocksError> {
    let mut head = [0u8; 2];
    stream.read_exact(&mut head)?;
    if head[0] != VERSION {
        return Err(SocksError::NotSocks5);
    }
    let mut methods = vec![0u8; head[1] as usize];
    stream.read_exact(&mut methods)?;
    match required {
        Some(creds) => {
            if !methods.contains(&METHOD_PASSWORD) {
                stream.write_all(&[VERSION, METHOD_NO_ACCEPTABLE])?;
                return Err(SocksError::NoAcceptableMethod);
            }
            stream.write_all(&[VERSION, METHOD_PASSWORD])?;
            let mut v = [0u8; 2];
            stream.read_exact(&mut v)?;
            if v[0] != AUTH_VERSION {
                return Err(SocksError::NotSocks5);
            }
            let mut user = vec![0u8; v[1] as usize];
            stream.read_exact(&mut user)?;
            let mut plen = [0u8; 1];
            stream.read_exact(&mut plen)?;
            let mut pass = vec![0u8; plen[0] as usize];
            stream.read_exact(&mut pass)?;
            let ok = creds.matches(&String::from_utf8_lossy(&user), &String::from_utf8_lossy(&pass));
            stream.write_all(&[AUTH_VERSION, if ok { 0 } else { 1 }])?;
            if !ok {
                return Err(SocksError::AuthFailed);
            }
        }
        None => {
            if !methods.contains(&METHOD_NONE) {
                stream.write_all(&[VERSION, METHOD_NO_ACCEPTABLE])?;
                return Err(SocksError::NoAcceptableMethod);
            }
            stream.write_all(&[VERSION, METHOD_NONE])?;
        }
    }
    let mut req = [0u8; 4];
    stream.read_exact(&mut req)?;
    if req[0] != VERSION {
        return Err(SocksError::NotSocks5);
    }
    if req[1] != CMD_CONNECT {
        server_reply(stream, reply::COMMAND_NOT_SUPPORTED)?;
        return Err(SocksError::Refused(reply::COMMAND_NOT_SUPPORTED));
    }
    match read_address(stream, req[3]) {
        Ok(a) => Ok(a),
        Err(SocksError::BadAddress(m)) => {
            server_reply(stream, reply::ADDRESS_NOT_SUPPORTED)?;
            Err(SocksError::BadAddress(m))
        }
        Err(e) => Err(e),
    }
}

/// Server side, step two: the answer to a request. The bound address is always `0.0.0.0:0`
/// (the client has no use for it, and it must not reveal the route).
pub fn server_reply<W: Write>(stream: &mut W, code: u8) -> io::Result<()> {
    let ip = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let mut out = vec![VERSION, code, 0];
    write_address(&mut out, &Address::Ip(SocketAddr::new(ip, 0))).map_err(|_| io::ErrorKind::InvalidInput)?;
    stream.write_all(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A pair of in-memory pipes would need threads; script one side's bytes instead.
    struct Script {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Script {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.extend(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn client_sends_domain_names_not_addresses() {
        // server: method none, then success with a bound IPv4 address
        let reply = vec![5, 0, 5, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        let mut s = Script { input: Cursor::new(reply), output: vec![] };
        client_connect(&mut s, &Address::Domain("example.com".into(), 443), None).expect("connects");
        assert_eq!(&s.output[..3], &[5, 1, 0]);
        let req = &s.output[3..];
        assert_eq!(&req[..4], &[5, 1, 0, ATYP_DOMAIN]);
        assert_eq!(req[4] as usize, "example.com".len());
        assert_eq!(&req[5..16], b"example.com");
        assert_eq!(&req[16..], &443u16.to_be_bytes());
    }

    #[test]
    fn client_logs_in_and_reports_refusals() {
        let reply = vec![5, 2, 1, 0, 5, 5, 0, 1, 0, 0, 0, 0, 0, 0];
        let mut s = Script { input: Cursor::new(reply), output: vec![] };
        let creds = Credentials::new("bob", "pw");
        let err = client_connect(&mut s, &Address::Domain("a.b".into(), 1), Some(&creds));
        assert!(matches!(err, Err(SocksError::Refused(5))));
        assert_eq!(&s.output[..4], &[5, 2, 0, 2]);
        assert_eq!(&s.output[4..12], &[1, 3, b'b', b'o', b'b', 2, b'p', b'w']);
        let mut bad = Script { input: Cursor::new(vec![5, 2, 1, 1]), output: vec![] };
        assert!(matches!(
            client_connect(&mut bad, &Address::Domain("a.b".into(), 1), Some(&creds)),
            Err(SocksError::AuthFailed)
        ));
        let mut http = Script { input: Cursor::new(b"HTTP/1.1 400".to_vec()), output: vec![] };
        assert!(matches!(
            client_connect(&mut http, &Address::Domain("a.b".into(), 1), None),
            Err(SocksError::NotSocks5)
        ));
    }

    #[test]
    fn server_parses_requests_and_checks_login() {
        let creds = Credentials::new("u", "p");
        let mut input = vec![5, 1, 2, 1, 1, b'u', 1, b'p', 5, 1, 0, ATYP_DOMAIN, 3];
        input.extend(b"a.b");
        input.extend(80u16.to_be_bytes());
        let mut s = Script { input: Cursor::new(input), output: vec![] };
        let addr = server_handshake(&mut s, Some(&creds)).expect("handshake");
        assert_eq!(addr, Address::Domain("a.b".into(), 80));
        assert_eq!(s.output, [5, 2, 1, 0]);

        let mut wrong = Script { input: Cursor::new(vec![5, 1, 2, 1, 1, b'u', 1, b'x']), output: vec![] };
        assert!(matches!(server_handshake(&mut wrong, Some(&creds)), Err(SocksError::AuthFailed)));
        assert_eq!(wrong.output, [5, 2, 1, 1]);

        let mut none = Script { input: Cursor::new(vec![5, 1, 0]), output: vec![] };
        assert!(matches!(server_handshake(&mut none, Some(&creds)), Err(SocksError::NoAcceptableMethod)));
        assert_eq!(none.output, [5, 0xFF]);

        let mut bind =
            Script { input: Cursor::new(vec![5, 1, 0, 5, 2, 0, 1, 1, 1, 1, 1, 0, 80]), output: vec![] };
        assert!(server_handshake(&mut bind, None).is_err());
        assert_eq!(bind.output[2..4], [5, reply::COMMAND_NOT_SUPPORTED]);
    }

    #[test]
    fn long_names_are_refused() {
        let mut out = Vec::new();
        assert!(write_address(&mut out, &Address::Domain("a".repeat(256), 1)).is_err());
        assert!(write_address(&mut out, &Address::Domain(String::new(), 1)).is_err());
    }
}
