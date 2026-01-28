use core_affinity;
use rtrb::RingBuffer;
use std::thread;
use std::time::{Duration, Instant};
use std::net::ToSocketAddrs;
use std::sync::Arc;
use rustls::{ClientConfig, RootCertStore};

use mio::{Events, Poll, Token};

// Modules
mod core;
mod net;
mod strategy;
mod ipc;
mod auth;

use net::ws_client::WsClient;
use net::framing; 
use core::orderbook::L2OrderBook;
use strategy::market_maker::{MarketMaker, ActionType};
use strategy::risk::RiskEngine;
use auth::signer::Signer;

#[derive(Debug, Clone, Copy)]
struct LogMessage {
    timestamp: u64,
    msg_type: u8, 
    value: f64, 
}

#[derive(PartialEq)]
enum ConnectionState {
    HandshakeSending,
    HandshakeWaiting,
    Subscribing,
    Active,
}

fn main() {
    println!("Initializing HFT Engine for AWS c8i.large (Cross-Platform Ready)...");
    
    #[cfg(target_os = "windows")]
    println!("Mode: Windows (Dev). Latency optimizations relaxed.");
    
    #[cfg(target_os = "linux")]
    println!("Mode: Linux (Prod). HFT optimizations ACTIVE.");

    let _ = rustls::crypto::ring::default_provider().install_default();

    // 1. Setup IPC
    let (mut producer, mut consumer) = RingBuffer::<LogMessage>::new(4096);

    let core_ids = core_affinity::get_core_ids().expect("Failed to retrieve core IDs");
    
    // On average laptop we might have many cores, but let's stick to 0 and 1.
    // Ensure we don't crash if only 1 core.
    let cold_core = if core_ids.len() > 1 { core_ids[1] } else { core_ids[0] };
    
    // COLD THREAD (Logger)
    let cold_handle = thread::spawn(move || {
        // Safe Pinning
        if core_affinity::set_for_current(cold_core) {
            println!("COLD Thread pinned to Core ID: {:?}", cold_core);
        } else {
            eprintln!("WARNING: Failed to pin COLD thread (Windows scheduler restriction?)");
        }

        println!("COLD Thread running.");
        loop {
             while let Ok(msg) = consumer.pop() {
                 if msg.msg_type == 10 { // Buy
                     println!("[SIMULATION] !!! SIGNAL TRIGGERED: BUY at {:.2} !!!", msg.value);
                 } else if msg.msg_type == 11 { // Sell
                     println!("[SIMULATION] !!! SIGNAL TRIGGERED: SELL at {:.2} !!!", msg.value);
                 } else if msg.msg_type == 1 { // Generic (fallback)
                     println!("[ACTION] TS:{} Val:{:.4}", msg.timestamp, msg.value);
                 }
             }
             thread::sleep(Duration::from_millis(1));
        }
    });

    // HOT THREAD (Strategy)
    let hot_core = core_ids[0]; 
    
    let hot_handle = thread::spawn(move || {
        if core_affinity::set_for_current(hot_core) {
            println!("HOT Thread pinned to Core ID: {:?}", hot_core);
        } else {
            eprintln!("WARNING: Failed to pin HOT thread (Windows scheduler restriction?)");
        }
        
        println!("HOT Thread running.");

        // --- INIT ---
        let mut book = L2OrderBook::new();
        // Strategy is now mutable
        let mut strategy = MarketMaker::new(1.0); 
        let mut risk = RiskEngine::new();
        
        let api_key = "TEST_API_KEY";
        let signer = Signer::new("TEST_SECRET_KEY");

        // --- NETWORK SETUP ---
        println!("HOT: Loading TLS...");
        let mut root_store = RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        
        let config = Arc::new(ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth());

        let host = "stream.bybit.com";
        let path = "/v5/public/linear";
        let addr = format!("{}:443", host).to_socket_addrs().unwrap().next().unwrap();
        
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(128); 
        
        let mut ws_client = match WsClient::connect(addr, host, config) {
            Ok(client) => client,
            Err(e) => {
                eprintln!("CRITICAL ERROR: Failed to connect to Bybit: {}", e);
                return;
            }
        };
        
        ws_client.register(poll.registry()).expect("Failed to register");
        // Initial Interest
        let interest = mio::Interest::READABLE | mio::Interest::WRITABLE;
        poll.registry().reregister(ws_client.tls.socket(), Token(0), interest).expect("Failed to reregister");

        let mut buf = [0u8; 65536];
        let mut write_buf = [0u8; 1024]; 
        let mut frame_buf = [0u8; 256]; 
        let mut signature_hex = [0u8; 64];

        let mut tick_count: u64 = 0;
        let mut state = ConnectionState::HandshakeSending;
        
        let mut offset = 0;

        println!("HOT: Entering Main Loop...");
        
        loop {
            if let Err(e) = poll.poll(&mut events, Some(Duration::from_millis(1))) {
                eprintln!("Poll error: {}", e);
            }

            for event in &events {
                match event.token() {
                    Token(0) => {
                        if event.is_writable() {
                            ws_client.is_connected = true; 
                            
                            match state {
                                ConnectionState::HandshakeSending => {
                                    println!("HOT: Sending Handshake...");
                                    if let Err(e) = ws_client.send_handshake(host, path) {
                                         eprintln!("Handshake send error: {}", e);
                                    }
                                    state = ConnectionState::HandshakeWaiting;
                                }
                                ConnectionState::Subscribing => {
                                    let sub_msg = r#"{"op": "subscribe", "args": ["orderbook.50.BTCUSDT"]}"#;
                                    println!("HOT: Sending Subscription: {}", sub_msg);
                                    
                                    let frame_len = framing::encode_text_frame(sub_msg.as_bytes(), &mut frame_buf);
                                    
                                    if frame_len > 0 {
                                        if let Err(e) = ws_client.tls.write_plaintext(&frame_buf[..frame_len]) {
                                            eprintln!("Subscription send error: {}", e);
                                        }
                                    }
                                    state = ConnectionState::Active;
                                }
                                _ => {}
                            }

                            if let Err(e) = ws_client.write_tls() {
                                eprintln!("TLS Write Error: {}", e);
                            }
                        }

                        if event.is_readable() {
                            risk.update_packet_time();
                            let start_tick = Instant::now();
                            
                            // Safe offset handling: if the buffer is full without a newline, clear it
                            if offset >= buf.len() {
                                eprintln!("HOT: Buffer overflow (no newline found in 64kb), resetting buffer.");
                                offset = 0;
                            }

                            match ws_client.read(&mut buf[offset..]) {
                                Ok(n) if n > 0 => {
                                    // DEBUG LOGGING
                                    #[cfg(debug_assertions)]
                                    println!("DEBUG: Received {} bytes", n);
                                    
                                    let end = offset + n;

                                    match state {
                                        ConnectionState::HandshakeWaiting => {
                                            // Handshake logic remains the same: assuming it fits in one read
                                            if let Ok(s) = std::str::from_utf8(&buf[..end]) {
                                                #[cfg(debug_assertions)]
                                                println!("DEBUG RESPONSE: {:.100}...", s);
                                                if s.contains("101 Switching Protocols") {
                                                    println!("HOT: WebSocket Upgraded! Ready to Subscribe.");
                                                    state = ConnectionState::Subscribing;
                                                    // Consume buffer
                                                    offset = 0; 
                                                } else {
                                                    // Keep accumulating for handshake? Usually it's short.
                                                    offset = end; 
                                                }
                                            }
                                        }
                                        ConnectionState::Active => {
                                            // Process all complete frames in buffer[0..end]
                                            let mut current_pos = 0;
                                            
                                            // We must loop as long as we can decode frames
                                            loop {
                                                // Slice from current position to end of valid data
                                                let slice = &mut buf[current_pos..end];
                                                
                                                match framing::decode_frame(slice) {
                                                    Ok(Some((consumed, payload))) => {
                                                        // "consumed" is the total frame length (header + payload)
                                                        // "payload" is the mutable slice of the actual data
                                                        
                                                        // Bybit usually sends JSON in Text frames.
                                                        // The payload doesn't necessarily have a newline. 
                                                        // It IS the JSON message.
                                                        
                                                        // We can parse it directly!
                                                        if !payload.is_empty() {
                                                             #[cfg(debug_assertions)]
                                                             println!("DEBUG: Frame decoded. Payload len: {}", payload.len());

                                                             match core::parser::parse_and_update(payload, &mut book) {
                                                                Ok(_) => {
                                                                    if let Some(action) = strategy.on_tick(&book) {
                                                                        // ... Strategy Execution Logic ...
                                                                        let latency = start_tick.elapsed();
                                                                        if latency.as_micros() > 50 {
                                                                            println!("[PERF] Tick-to-Trade Latency: {}µs", latency.as_micros());
                                                                        }

                                                                        let side = match action.action_type {
                                                                            ActionType::LimitBuy => "Buy",
                                                                            ActionType::LimitSell => "Sell",
                                                                            _ => "Buy"
                                                                        };
                                                                        
                                                                        let log_type = match action.action_type {
                                                                            ActionType::LimitBuy => 10,
                                                                            ActionType::LimitSell => 11,
                                                                            _ => 1
                                                                        };
                                                                        
                                                                        let symbol = "BTCUSDT";
                                                                        
                                                                        let json_len = core::serializer::write_order_json(
                                                                            &mut write_buf, 
                                                                            symbol, 
                                                                            side, 
                                                                            action.qty, 
                                                                            action.price
                                                                        );
                                                                        
                                                                        let payload = &write_buf[..json_len];
                                                                        let ts = 1672304486868; 
                                                                        signer.sign_request(ts, api_key, 5000, payload, &mut signature_hex);
                                                                        
                                                                        let _ = producer.push(LogMessage {
                                                                            timestamp: tick_count,
                                                                            msg_type: log_type, 
                                                                            value: action.price
                                                                        });
                                                                    }
                                                                },
                                                                Err(_e) => {
                                                                     // eprintln!("PARSER ERROR: {:?}", _e);
                                                                     // Might be control frame payload or unexpected data
                                                                }
                                                            }
                                                        }
                                                        
                                                        current_pos += consumed;
                                                    },
                                                    Ok(None) => {
                                                        // Incomplete frame, wait for more data
                                                        break;
                                                    },
                                                    Err(e) => {
                                                        eprintln!("FRAME ERROR: {}", e);
                                                        // If error, we probably lost sync or got bad data.
                                                        // Hard reset connection or skip?
                                                        // For now, maybe skip 1 byte? Or better, reset connection logic.
                                                        // But let's just break and consume what we can or wait.
                                                        // If we are stuck, we might loop forever if we don't consume.
                                                        // Force consume 1 byte to try resync? (Risky)
                                                        // Let's break and hope next read fixes it or reconnect.
                                                        break; 
                                                    }
                                                }
                                            }

                                            // Copy tail to beginning
                                            if current_pos < end {
                                                let tail_len = end - current_pos;
                                                buf.copy_within(current_pos..end, 0);
                                                offset = tail_len;
                                            } else {
                                                offset = 0;
                                            }
                                        }
                                        _ => {
                                             // For other states (e.g. sending), just consume or accumulate?
                                             // Ideally we shouldn't be reading much in sending, but if we do, clear it to avoid overflow
                                             offset = 0; 
                                        }
                                    }
                                    
                                    risk.check_internal_latency(start_tick);
                                }
                                Ok(_) => {}, 
                                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {},
                                Err(e) => eprintln!("HOT: IO Error: {}", e),
                            }
                        }
                    }
                    _ => {}
                }
            }
            
            // DYNAMIC REGISTRATION
            let interest = if ws_client.tls.wants_write() || state == ConnectionState::HandshakeSending || state == ConnectionState::Subscribing {
                mio::Interest::READABLE | mio::Interest::WRITABLE
            } else {
                mio::Interest::READABLE
            };
            
            // Re-register to update interests if needed
            // NOTE: In strict MIO edge-triggered, this is okay.
            // But checking every tick might be heavy. 
            // For MVP (latency check logic) we only care if state changed.
             poll.registry().reregister(ws_client.tls.socket(), Token(0), interest).unwrap();

            tick_count = tick_count.wrapping_add(1);
        }
    });

    hot_handle.join().unwrap();
    cold_handle.join().unwrap();
}
