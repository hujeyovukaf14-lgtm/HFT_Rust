use core_affinity;
use rtrb::RingBuffer;
use std::thread;
use std::time::Duration;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use rustls::{ClientConfig, RootCertStore};

use mio::{Events, Poll, Token};

// Modules
mod core;
mod net;
mod strategy;
mod ipc;

use net::ws_client::WsClient;

#[derive(Debug, Clone, Copy)]
struct LogMessage {
    timestamp: u64,
    msg_type: u8, 
    value: f64, 
}

fn main() {
    println!("Initializing HFT Engine for AWS c8i.large...");

    // Install default crypto provider (Ring)
    // This is required for ClientConfig::builder() to work with defaults in 0.23+
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 1. Setup SPSC RingBuffer
    let (mut producer, mut consumer) = RingBuffer::<LogMessage>::new(4096);

    let core_ids = core_affinity::get_core_ids().expect("Failed to retrieve core IDs");
    println!("Detected {} cores.", core_ids.len());

    let cold_core = if core_ids.len() > 1 { core_ids[1] } else { core_ids[0] };
    
    // COLD THREAD
    let cold_handle = thread::spawn(move || {
        if core_affinity::set_for_current(cold_core) {
            println!("COLD Thread pinned to Core ID: {:?}", cold_core);
        }
        loop {
             if let Ok(_msg) = consumer.pop() {}
             thread::sleep(Duration::from_millis(1));
        }
    });

    // HOT THREAD
    let hot_core = core_ids[0]; 
    
    let hot_handle = thread::spawn(move || {
        if core_affinity::set_for_current(hot_core) {
            println!("HOT Thread pinned to Core ID: {:?}", hot_core);
        }

        println!("HOT: Loading TLS Root Certificates...");
        let mut root_store = RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        
        // Build ClientConfig
        // Now that provider is installed, builder() should use it for protocol versions/ciphers
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
            
        let config_arc = Arc::new(config);

        // Bybit Testnet Host
        let host = "stream-testnet.bybit.com";
        let path = "/v5/public/linear";
        let addr_str = format!("{}:443", host);
        let addr = addr_str.to_socket_addrs().unwrap().next().unwrap();
        
        println!("HOT: Connecting to {} ({:?})...", addr_str, addr);
        
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(128);
        
        let mut ws_client = WsClient::connect(addr, host, config_arc).expect("Failed to connect");
        ws_client.register(poll.registry()).expect("Failed to register poll");

        let mut buf = [0u8; 4096];
        let mut tick_count: u64 = 0;

        println!("HOT: Starting Event Loop...");
        
        loop {
            // Poll for events
            poll.poll(&mut events, Some(Duration::from_millis(1))).unwrap();

            for event in &events {
                match event.token() {
                    Token(0) => {
                        if event.is_writable() {
                            if !ws_client.is_connected {
                                println!("HOT: Socket Writable (TCP connected)");
                                ws_client.is_connected = true;
                                println!("HOT: Sending WS Handshake...");
                                ws_client.send_handshake(host, path).unwrap();
                            }
                            
                            if let Err(e) = ws_client.write_tls() {
                                if e.kind() != std::io::ErrorKind::WouldBlock {
                                     eprintln!("TLS Write Error: {}", e);
                                }
                            }
                        }

                        if event.is_readable() {
                            match ws_client.read(&mut buf) {
                                Ok(n) => {
                                    if n > 0 {
                                        let s = std::str::from_utf8(&buf[..n]).unwrap_or("<binary/encrypted>");
                                        println!("HOT: Recv {} bytes: {:.50}...", n, s);
                                        let _ = producer.push(LogMessage { timestamp: tick_count, msg_type: 2, value: 0.0 });
                                    }
                                }
                                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {},
                                Err(e) => eprintln!("HOT: Read error: {}", e),
                            }
                        }
                    }
                    _ => {}
                }
            }
            tick_count = tick_count.wrapping_add(1);
        }
    });

    hot_handle.join().unwrap();
    cold_handle.join().unwrap();
}
