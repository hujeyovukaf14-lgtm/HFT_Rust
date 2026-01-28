use core_affinity;
use rtrb::RingBuffer;
use std::thread;
use std::time::{Duration, Instant};
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::io::Write; // Import Write for flush
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
use simd_json; 
use simd_json::prelude::*;

#[derive(Debug, Clone, Copy)]
struct LogMessage {
    timestamp: u64,
    msg_type: u8, 
    bybit_bid: f64,
    bybit_ask: f64,
    binance_bid: f64,
    binance_ask: f64,
    latency: u64,
}

#[derive(PartialEq)]
enum ConnectionState {
    HandshakeSending,
    HandshakeWaiting,
    Subscribing,
    Authenticating,
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
                 if msg.msg_type == 1 || msg.msg_type == 20 { // Status Update OR Quote Adjustment
                     // Calculate spread for display
                     let spread_diff = (msg.binance_bid - msg.bybit_ask).max(msg.bybit_bid - msg.binance_ask);
                     
                     // Marker for action
                     let action_marker = if msg.msg_type == 20 { "[QUOTE]" } else { "       " };

                     print!("\rBybit: {:.2}/{:.2} | Binance: {:.2}/{:.2} | Diff: {:.2} | Latency: {}us {}   ", 
                         msg.bybit_bid, msg.bybit_ask, 
                         msg.binance_bid, msg.binance_ask, 
                         spread_diff,
                         msg.latency,
                         action_marker
                     );
                     let _ = std::io::stdout().flush();
                 } else if msg.msg_type == 10 { // Buy Signal
                     println!("\n[SIMULATION] !!! SIGNAL TRIGGERED: BUY (Skewed Quote) !!!");
                 } else if msg.msg_type == 11 { // Sell Signal
                     println!("\n[SIMULATION] !!! SIGNAL TRIGGERED: SELL (Skewed Quote) !!!");
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
        let mut strategy = MarketMaker::new(0.01); 
        let mut risk = RiskEngine::new();
        let mut last_latency = 0; // Track last execution latency
        
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
        
        let mut ws_client = match WsClient::connect(addr, host, config.clone()) {
            Ok(client) => client,
            Err(e) => {
                eprintln!("CRITICAL ERROR: Failed to connect to Bybit: {}", e);
                return;
            }
        };

        // --- BINANCE SETUP ---
        let bin_host = "fstream.binance.com";
        let bin_path = "/ws/btcusdt@bookTicker"; 
        let bin_addr = format!("{}:443", bin_host).to_socket_addrs().unwrap().next().unwrap();
        
        let mut ws_binance = match WsClient::connect(bin_addr, bin_host, config.clone()) {
            Ok(client) => client,
            Err(e) => {
                eprintln!("CRITICAL ERROR: Failed to connect to Binance: {}", e);
                return;
            }
        };
        
        // --- PRIVATE BYBIT SETUP ---
        let priv_host = "stream.bybit.com";
        let priv_path = "/v5/private";
        let priv_addr = format!("{}:443", priv_host).to_socket_addrs().unwrap().next().unwrap();
        
        let mut ws_private = match WsClient::connect(priv_addr, priv_host, config.clone()) {
            Ok(client) => client,
            Err(e) => {
                eprintln!("CRITICAL ERROR: Failed to connect to Bybit Private: {}", e);
                return;
            }
        };

        // Tokens
        const BYBIT_TOKEN: Token = Token(0);
        const BINANCE_TOKEN: Token = Token(1);
        const BYBIT_PRIVATE_TOKEN: Token = Token(2);
    
        // Register All
        ws_client.register(poll.registry(), BYBIT_TOKEN).expect("Failed to register Bybit Public");
        ws_binance.register(poll.registry(), BINANCE_TOKEN).expect("Failed to register Binance");
        ws_private.register(poll.registry(), BYBIT_PRIVATE_TOKEN).expect("Failed to register Bybit Private");
    
        // Buffers for Binance
        let mut bin_buf = [0u8; 65536];
        let mut bin_offset = 0;
        let mut bin_handshake_done = false;
    
        // Buffers for Bybit
        let mut buf = [0u8; 65536];
        let mut offset = 0; 
    
        // Reuse other buffers
        let mut write_buf = [0u8; 1024]; 
        let mut frame_buf = [0u8; 256]; 
        let mut signature_hex = [0u8; 64];
    
        // Buffers for Private
        let mut priv_buf = [0u8; 65536];
        let mut priv_offset = 0;
        
        let mut tick_count: u64 = 0;
        let mut state = ConnectionState::HandshakeSending;
        let mut priv_state = ConnectionState::HandshakeSending;
        
        let mut bin_active = false;
        
        // Auto-Liquidation State
        let mut last_fill_ts: Option<Instant> = None;
    
        println!("HOT: Entering Main Loop (Dual Exchange Mode)...");
        
        loop {
        if let Err(e) = poll.poll(&mut events, Some(Duration::from_millis(1))) {
            eprintln!("Poll error: {}", e);
        }

        for event in &events {
            match event.token() {
                BYBIT_TOKEN => {
                    if event.is_writable() {
                        ws_client.is_connected = true; 
                        
                        match state {
                            ConnectionState::HandshakeSending => {
                                println!("HOT: Sending Bybit Handshake...");
                                if let Err(e) = ws_client.send_handshake(host, path) {
                                     eprintln!("Bybit Handshake send error: {}", e);
                                }
                                state = ConnectionState::HandshakeWaiting;
                            }
                            ConnectionState::Subscribing => {
                                let sub_msg = r#"{"op": "subscribe", "args": ["orderbook.50.RIVERUSDT"]}"#;
                                println!("HOT: Sending Bybit Subscription: {}", sub_msg);
                                
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
                            eprintln!("Bybit TLS Write Error: {}", e);
                        }
                    }

                    if event.is_readable() {
                        risk.update_packet_time();
                        let start_tick = Instant::now();
                        
                        // BYBIT READ logic
                        if offset >= buf.len() {
                             offset = 0; // Reset on overflow
                        }
                        
                        match ws_client.read(&mut buf[offset..]) {
                            Ok(n) if n > 0 => {
                                let end = offset + n;
                                match state {
                                    ConnectionState::HandshakeWaiting => {
                                        if let Ok(s) = std::str::from_utf8(&buf[..end]) {
                                            if s.contains("101 Switching Protocols") {
                                                println!("HOT: Bybit Upgraded! Ready to Subscribe.");
                                                state = ConnectionState::Subscribing;
                                                offset = 0; 
                                            } else {
                                                offset = end; 
                                            }
                                        }
                                    }
                                    ConnectionState::Active => {
                                        // Frame Decoding Loop
                                        let mut current_pos = 0;
                                        loop {
                                            let slice = &mut buf[current_pos..end];
                                            match framing::decode_frame(slice) {
                                                Ok(Some((consumed, payload))) => {
                                                    if !payload.is_empty() {
                                                         // Parse Bybit
                                                         if let Ok(_) = core::parser::parse_and_update(payload, &mut book) {
                                                             // Trigger Strategy
                                                             if let Some(actions) = strategy.on_tick(&book) {
                                                                 // Loop through actions
                                                                 for action in actions {
                                                                     // Send to Private WS
                                                                     let req_json = match action.action_type {
                                                                         ActionType::CreateOrder { price, qty, side, link_id } => {
                                                                             format!(r#"{{"op":"order.create","args":[{{ "symbol":"RIVERUSDT","side":"{}","orderType":"Limit","qty":"{}","price":"{}","timeInForce":"PostOnly","orderLinkId":"{}" }}]}}"#, 
                                                                                 side, qty, price, link_id)
                                                                         },
                                                                         ActionType::AmendOrder { price, qty, side: _, link_id } => {
                                                                             format!(r#"{{"op":"order.amend","args":[{{ "symbol":"RIVERUSDT","qty":"{}","price":"{}","orderLinkId":"{}" }}]}}"#, 
                                                                                 qty, price, link_id)
                                                                         },
                                                                         _ => String::new()
                                                                     };
                                                                     
                                                                     let latency = start_tick.elapsed();
                                                                     let lat_u64 = latency.as_micros() as u64;

                                                                     if !req_json.is_empty() {
                                                                         let _ = framing::encode_text_frame(req_json.as_bytes(), &mut frame_buf);
                                                                         // Write to PRIVATE WS
                                                                         // We assume Private WS is ready. Ideally check state.
                                                                         if let Err(e) = ws_private.tls.write_plaintext(&frame_buf[..req_json.len() + 20]) { 
                                                                             eprintln!("Order Send Error: {}", e);
                                                                         } else {
                                                                             // Hack: Force queue write
                                                                             let _ = ws_private.write_tls();
                                                                         }
                                                                     }
                                                                     
                                                                     // Push Log
                                                                     let _ = producer.push(LogMessage {
                                                                         timestamp: tick_count,
                                                                         msg_type: 20, 
                                                                         bybit_bid: book.bids[0].price,
                                                                         bybit_ask: book.asks[0].price,
                                                                         binance_bid: strategy.binance_bid,
                                                                         binance_ask: strategy.binance_ask,
                                                                         latency: lat_u64,
                                                                     });
                                                                 }
                                                             }
                                                         }
                                                    }

                                                    // Throttled Status Update (every 100 ticks)
                                                    if tick_count % 100 == 0 {
                                                         let _ = producer.push(LogMessage {
                                                             timestamp: tick_count,
                                                             msg_type: 1, // Status
                                                             bybit_bid: book.bids[0].price,
                                                             bybit_ask: book.asks[0].price,
                                                             binance_bid: strategy.binance_bid,
                                                             binance_ask: strategy.binance_ask,
                                                             latency: last_latency as u64,
                                                         });
                                                    }
                                                    current_pos += consumed;
                                                },
                                                Ok(None) => break,
                                                Err(_) => break, // Drop invalid
                                            }
                                        }
                                        if current_pos < end {
                                            buf.copy_within(current_pos..end, 0);
                                            offset = end - current_pos;
                                        } else {
                                            offset = 0;
                                        }
                                    }
                                    _ => { offset = 0; }
                                }
                                risk.check_internal_latency(start_tick);
                            }
                            Ok(_) => {},
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {},
                            Err(e) => eprintln!("HOT: Bybit IO Error: {}", e),
                        }
                    }
                }
                
                BINANCE_TOKEN => {
                     // BINANCE LOGIC
                     if event.is_writable() && !bin_handshake_done {
                         println!("HOT: Sending Binance Handshake...");
                         if let Err(e) = ws_binance.send_handshake(bin_host, bin_path) {
                              eprintln!("Binance Handshake Error: {}", e);
                         }
                         bin_handshake_done = true; 
                     }
                     if event.is_writable() {
                         let _ = ws_binance.write_tls();
                     }

                     if event.is_readable() {
                         let start_tick = Instant::now();
                         if bin_offset >= bin_buf.len() { bin_offset = 0; }
                         match ws_binance.read(&mut bin_buf[bin_offset..]) {
                             Ok(n) if n > 0 => {
                                 let end = bin_offset + n;
                                 
                                 if !bin_active {
                                     // Check for Handshake Response (Raw HTTP)
                                     if let Ok(s) = std::str::from_utf8(&bin_buf[..end]) {
                                         if s.contains("101 Switching Protocols") {
                                             println!("HOT: Binance Upgraded!");
                                             bin_active = true;
                                             // Reset buffer (consumed handshake)
                                             bin_offset = 0; 
                                         } else {
                                             // Keep accumulating
                                             bin_offset = end;
                                         }
                                     }
                                 } else {
                                     // Active - Decode Frames
                                     let mut current_pos = 0;
                                     loop {
                                         let slice = &mut bin_buf[current_pos..end];
                                         match framing::decode_frame(slice) {
                                             Ok(Some((consumed, payload))) => {
                                                 if !payload.is_empty() {
                                                     // if let Ok(s) = std::str::from_utf8(payload) {
                                                     //      println!("DEBUG: Binance RAW: {:.50}...", s);
                                                     // }

                                                     // Parse "b" and "a"
                                                     // Format: {"u":.., "s":"ETHUSDT", "b":"...", "a":"...", ...}
                                                     if let Ok(json) = simd_json::to_borrowed_value(payload) {
                                                         if let (Some(b_str), Some(a_str)) = (
                                                             json.get("b").and_then(|v| v.as_str()), 
                                                             json.get("a").and_then(|v| v.as_str())
                                                         ) {
                                                             if let (Ok(bid), Ok(ask)) = (b_str.parse::<f64>(), a_str.parse::<f64>()) {
                                                                 // println!("DEBUG: Binance Book: {} / {}", bid, ask); // Uncomment if needed
                                                                 strategy.update_binance_price(bid, ask);
                                                                 
                                                                 // Trigger arb check immediately
                                                                 // Trigger arb check immediately
                                                                 // Trigger Strategy (Active Maker Logic)
                                                                 if let Some(_) = strategy.on_tick(&book) {
                                                                     // Ignore actions from Binance triggers for now to avoid double-firing, 
                                                                     // OR implement same logic. 
                                                                     // Strategy rate limiter handles 1s anyway.
                                                                     // Ideally we just update price.
                                                                 } else {
                                                                     // No Signal - Push Status Update (High Freq? Maybe throttle?)
                                                                     let _ = producer.push(LogMessage {
                                                                         timestamp: tick_count,
                                                                         msg_type: 1, // Status
                                                                         bybit_bid: book.bids[0].price,
                                                                         bybit_ask: book.asks[0].price,
                                                                         binance_bid: bid, 
                                                                         binance_ask: ask,
                                                                         latency: last_latency as u64,
                                                                     });
                                                                 }
                                                             }
                                                         }
                                                     }
                                                 }
                                                 current_pos += consumed;
                                             },
                                             Ok(None) => break,
                                             Err(_) => break,
                                         }
                                     }
                                     if current_pos < end {
                                        bin_buf.copy_within(current_pos..end, 0);
                                        bin_offset = end - current_pos;
                                    } else {
                                        bin_offset = 0;
                                    }
                                 }
                             }
                             Ok(_) => {},
                             Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {},
                             Err(e) => eprintln!("HOT: Binance IO Error: {}", e),
                         }
                     }
                }

                BYBIT_PRIVATE_TOKEN => {
                    if event.is_writable() {
                         match priv_state {
                            ConnectionState::HandshakeSending => {
                                println!("HOT: Sending Private Handshake...");
                                if let Err(e) = ws_private.send_handshake(priv_host, priv_path) {
                                     eprintln!("Private Handshake send error: {}", e);
                                }
                                priv_state = ConnectionState::HandshakeWaiting;
                            }
                            ConnectionState::Authenticating => {
                                // Auth
                                let expires = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() + 5000;
                                let sign_payload = format!("GET/realtime{}", expires);
                                signer.sign_message(sign_payload.as_bytes(), &mut signature_hex);
                                let sig_str = std::str::from_utf8(&signature_hex[..64]).unwrap_or(""); 
                                
                                let auth_msg = format!(r#"{{"op":"auth","args":["{}","{}","{}"]}}"#, api_key, expires, sig_str);
                                println!("HOT: Authenticating Private WS...");
                                let _ = framing::encode_text_frame(auth_msg.as_bytes(), &mut frame_buf);
                                let _ = ws_private.tls.write_plaintext(&frame_buf[..auth_msg.len() + 20]); // Safety margin
                                priv_state = ConnectionState::Active; 
                            }
                            _ => {}
                        }
                        let _ = ws_private.write_tls();
                    }

                    if event.is_readable() {
                        if priv_offset >= priv_buf.len() { priv_offset = 0; }
                        match ws_private.read(&mut priv_buf[priv_offset..]) {
                            Ok(n) if n > 0 => {
                                let end = priv_offset + n;
                                match priv_state {
                                    ConnectionState::HandshakeWaiting => {
                                        if let Ok(s) = std::str::from_utf8(&priv_buf[..end]) {
                                            if s.contains("101 Switching Protocols") {
                                                println!("HOT: Private Switch Proto!");
                                                priv_state = ConnectionState::Authenticating; 
                                                priv_offset = 0;
                                            } else { priv_offset = end; }
                                        }
                                    }
                                    ConnectionState::Active => {
                                        // Parse Executions
                                        let mut current_pos = 0;
                                        loop {
                                            let slice = &mut priv_buf[current_pos..end];
                                            match framing::decode_frame(slice) {
                                                Ok(Some((consumed, payload))) => {
                                                    if !payload.is_empty() {
                                                        if let Ok(json) = simd_json::to_borrowed_value(payload) {
                                                             // Check for Execution
                                                             if let Some(topic) = json.get("topic").and_then(|v| v.as_str()) {
                                                                 if topic == "execution" {
                                                                     // Filled?
                                                                     println!("\n[EXECUTION] Trade Filled!");
                                                                     last_fill_ts = Some(Instant::now());
                                                                 }
                                                             }
                                                        }
                                                    }
                                                    current_pos += consumed;
                                                },
                                                Ok(None) => break,
                                                Err(_) => break,
                                            }
                                        }
                                        if current_pos < end {
                                             priv_buf.copy_within(current_pos..end, 0);
                                             priv_offset = end - current_pos;
                                        } else { priv_offset = 0; }
                                    }
                                    _ => { priv_offset = 0; }
                                }
                            }
                            Ok(_) => {},
                            Err(_) => {},
                        }
                    }
                    
                    // Auto-Liquidation Logic
                     if let Some(ts) = last_fill_ts {
                         if ts.elapsed() > Duration::from_millis(1000) {
                             println!("\n[RISK] Auto-Liquidation Triggered!");
                             let close_msg = r#"{"op":"order.create","args":[{"symbol":"RIVERUSDT","side":"Sell","orderType":"Market","qty":"15","reduceOnly":true}]}"#; // Hardcoded qty for safety
                             let _ = framing::encode_text_frame(close_msg.as_bytes(), &mut frame_buf);
                             let _ = ws_private.tls.write_plaintext(&frame_buf[..close_msg.len()+10]);
                             last_fill_ts = None;
                         }
                     }

                }
                _ => {}
            }
        }
        
        // Reregister Both (every loop might be heavy, but needed for TLS wants_write state?)
        // Only if state changes ideally.
        // For MVP, keep it simple.
        let bybit_interest = if ws_client.tls.wants_write() || state == ConnectionState::HandshakeSending || state == ConnectionState::Subscribing {
            mio::Interest::READABLE | mio::Interest::WRITABLE
        } else {
            mio::Interest::READABLE
        };
        poll.registry().reregister(ws_client.tls.socket(), BYBIT_TOKEN, bybit_interest).unwrap();

        let bin_interest = if ws_binance.tls.wants_write() || !bin_handshake_done {
            mio::Interest::READABLE | mio::Interest::WRITABLE
        } else {
            mio::Interest::READABLE
        };
        poll.registry().reregister(ws_binance.tls.socket(), BINANCE_TOKEN, bin_interest).unwrap();

        let priv_interest = if ws_private.tls.wants_write() || priv_state != ConnectionState::Active {
             mio::Interest::READABLE | mio::Interest::WRITABLE
        } else {
             mio::Interest::READABLE
        };
        poll.registry().reregister(ws_private.tls.socket(), BYBIT_PRIVATE_TOKEN, priv_interest).unwrap();

        tick_count = tick_count.wrapping_add(1);
    }
    });

    hot_handle.join().unwrap();
    cold_handle.join().unwrap();
}
