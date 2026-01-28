use mio::{Events, Interest, Poll, Token, Registry};
use std::io::{self, Read, Write, ErrorKind};
use std::net::SocketAddr;
use std::sync::Arc;
use rustls::ClientConfig;
use crate::net::tcp_opt;
use crate::net::tls_client::TlsClient;

// Token for our socket in the MIO poll
const WS_TOKEN: Token = Token(0);

pub struct WsClient {
    pub tls: TlsClient,
    pub is_connected: bool,
    pub handshake_complete: bool,
}

impl WsClient {
    // UPDATED: Now takes Arc<ClientConfig>
    pub fn connect(addr: SocketAddr, server_name: &str, config: Arc<ClientConfig>) -> io::Result<Self> {
        // 1. Create optimized raw socket
        let raw_socket = tcp_opt::create_socket()?;
        
        // 2. Initiate connection
        match raw_socket.connect(&addr.into()) {
            Ok(_) => {}
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {} // Expected
            Err(e) => return Err(e),
        }

        let mio_stream: mio::net::TcpStream = mio::net::TcpStream::from_std(raw_socket.into());
        
        // 3. Wrap in TLS
        let tls_client = TlsClient::new(mio_stream, server_name, config)?;

        Ok(Self {
            tls: tls_client,
            is_connected: false,
            handshake_complete: false,
        })
    }

    pub fn register(&mut self, registry: &Registry, token: Token) -> io::Result<()> {
         self.tls.register(registry, token)
    }

    pub fn send_handshake(&mut self, host: &str, path: &str) -> io::Result<()> {
        let request = format!(
            "GET {} HTTP/1.1\r\n\
             Host: {}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             \r\n",
            path, host
        );

        self.tls.write_plaintext(request.as_bytes())?;
        self.tls.write_tls()?;
        Ok(())
    }

    pub fn read<'a>(&mut self, buf: &'a mut [u8]) -> io::Result<usize> {
        self.tls.read(buf)
    }
    
    pub fn write_tls(&mut self) -> io::Result<()> {
        self.tls.write_tls()
    }
}
