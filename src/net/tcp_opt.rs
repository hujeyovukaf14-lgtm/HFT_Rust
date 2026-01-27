use std::io;
use std::net::TcpStream;
use socket2::{Socket, Domain, Type, Protocol};

/// Sets HFT-optimized TCP flags on a raw socket or TcpStream.
/// 
/// # Optimizations
/// * `TCP_NODELAY` (Disable Nagle's Algorithm): Sends data immediately, critical for order latency.
/// * `TCP_QUICKACK` (Linux only - usually): Tells OS to send ACK immediately, not delaying it. 
///   Note: On Windows this might be no-op or require specific handling, but we try standard API.
/// * `Non-blocking`: Essential for `mio` event loop.
pub fn apply_optimizations(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_nonblocking(true)?;
    // QuickAck is platform-specific and tricky in portable Rust, 
    // but Nodelay is the biggest win. 
    // We'll stick to std and mio capabilities for now to avoid unsafe libc calls if not strictly needed yet.
    Ok(())
}

/// Creates a properly configured socket for outbound connection.
/// 
/// Returns a `socket2::Socket` which can be converted to `std::net::TcpStream`.
pub fn create_socket() -> io::Result<Socket> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    
    // Set non-blocking before connect to allow async connect behavior
    socket.set_nonblocking(true)?;
    
    // Nodelay might need to be set after connect on some platforms, 
    // but setting it here is good practice if supported.
    socket.set_nodelay(true)?;
    
    Ok(socket)
}
