use std::io::{self, Read, Write};
use std::sync::Arc;
use mio::{Events, Interest, Poll, Token, Registry};
use mio::net::TcpStream;
use rustls::{ClientConnection, ClientConfig, RootCertStore, pki_types::ServerName};
use std::convert::TryFrom;
use crate::net::tcp_opt;

/// A non-blocking TLS wrapper around mio::net::TcpStream.
/// Designed for HFT: No internal Mutex/Locks. State is owned by the struct.
pub struct TlsClient {
    pub socket: TcpStream,
    pub tls_conn: ClientConnection,
}

impl TlsClient {
    pub fn new(socket: TcpStream, server_name: &str, config: Arc<ClientConfig>) -> io::Result<Self> {
        let server_name = ServerName::try_from(server_name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid DNS name"))?
            .to_owned();

        let tls_conn = ClientConnection::new(config, server_name)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        Ok(Self {
            socket,
            tls_conn,
        })
    }

    pub fn register(&mut self, registry: &Registry, token: Token) -> io::Result<()> {
        registry.register(&mut self.socket, token, Interest::READABLE | Interest::WRITABLE)
    }

    pub fn socket(&mut self) -> &mut TcpStream {
        &mut self.socket
    }

    pub fn wants_write(&self) -> bool {
        self.tls_conn.wants_write()
    }

    /// Pulls encrypted data from socket -> TLS Engine.
    /// Returns true if data was read.
    pub fn read_tls(&mut self) -> io::Result<bool> {
        match self.tls_conn.read_tls(&mut self.socket) {
            Ok(n) => {
                 let state = self.tls_conn.process_new_packets()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                 
                 // FIX: use state directly, it IS IoState
                 Ok(n > 0 || state.plaintext_bytes_to_read() > 0)
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Pushes encrypted data from TLS Engine -> Socket.
    pub fn write_tls(&mut self) -> io::Result<()> {
        if self.tls_conn.wants_write() {
             match self.tls_conn.write_tls(&mut self.socket) {
                 Ok(n) => {
                     // DEBUG:
                     println!("DEBUG: write_tls flushed {} bytes. Wants write: {}", n, self.tls_conn.wants_write());
                     Ok(())
                 },
                 Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
                 Err(e) => Err(e),
             }
        } else {
            Ok(())
        }
    }

    /// Reads PLAINTEXT from the internal TLS buffer into `buf`.
    pub fn read_plaintext(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let _ = self.read_tls()?; 
        match self.tls_conn.reader().read(buf) {
            Ok(n) => Ok(n),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Writes PLAINTEXT into the internal TLS buffer.
    pub fn write_plaintext(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.tls_conn.writer().write(buf) {
             Ok(n) => Ok(n),
             Err(e) => Err(e),
        }
    }
}
