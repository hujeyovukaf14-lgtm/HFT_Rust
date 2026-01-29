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

static MINIMAL_LOGS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

macro_rules! info {
    ($($arg:tt)*) => {
        if !crate::MINIMAL_LOGS.load(std::sync::atomic::Ordering::Relaxed) {
             println!($($arg)*);
        }
    }
}

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

#[derive(PartialEq, Debug)]
enum ConnectionState {
    HandshakeSending,
    HandshakeWaiting,
    Subscribing,
    Authenticating,
    Active,
}

// HTTP REST function to cancel all orders on startup
fn cancel_all_orders_http(api_key: &str, api_secret: &str) -> Result<(), String> {
    info!("========================================");
    info!(">>> Canceling ALL orders via HTTP REST...");
    
    let url = "https://api.bybit.com/v5/order/cancel-all";
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    
    // Subtract 6 seconds to account for clock skew (local clock is ahead)
    let timestamp = timestamp.saturating_sub(6000);
    
    let recv_window = 20000u64;
    
    // POST body as JSON
    let body = r#"{"category":"linear","symbol":"RIVERUSDT"}"#;
    
    // Signature string for POST: timestamp + api_key + recv_window + body
    let sign_str = format!("{}{}{}{}", timestamp, api_key, recv_window, body);
    
    // HMAC SHA256
    use ring::hmac;
    let key = hmac::Key::new(hmac::HMAC_SHA256, api_secret.as_bytes());
    let signature = hmac::sign(&key, sign_str.as_bytes());
    let sig_hex = hex::encode(signature.as_ref());
    
    // Make HTTP POST request with JSON body
    let response = ureq::post(url)
        .set("Content-Type", "application/json")
        .set("X-BAPI-API-KEY", api_key)
        .set("X-BAPI-TIMESTAMP", &timestamp.to_string())
        .set("X-BAPI-SIGN", &sig_hex)
        .set("X-BAPI-RECV-WINDOW", &recv_window.to_string())
        .send_string(body);
    
    match response {
        Ok(resp) => {
            let body = resp.into_string().map_err(|e| format!("Failed to read response: {}", e))?;
            info!(">>> CancelAll HTTP Response: {}", body);
            
            // Parse retCode
            if let Ok(json) = simd_json::to_borrowed_value(body.as_bytes().to_vec().as_mut_slice()) {
                let ret_code = json.get("retCode").and_then(|v| v.as_i64()).unwrap_or(-1);
                if ret_code == 0 {
                    info!(">>> CancelAll HTTP SUCCESS!");
                    info!("========================================");
                    Ok(())
                } else {
                    let ret_msg = json.get("retMsg").and_then(|v| v.as_str()).unwrap_or("unknown");
                    eprintln!("!!! CancelAll HTTP FAILED: {} - {}", ret_code, ret_msg);
                    Err(format!("Bybit error: {} - {}", ret_code, ret_msg))
                }
            } else {
                Err("Failed to parse response JSON".to_string())
            }
        },
        Err(e) => {
            eprintln!("!!! HTTP Request FAILED: {}", e);
            Err(format!("HTTP error: {}", e))
        }
    }
}



fn main() {
    if std::env::var("HFT_LOG_MODE").unwrap_or_default() == "minimal" {
         MINIMAL_LOGS.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    
    println!("Initializing HFT Engine"); 
    info!("Mode: Generic. Logs will be verbose unless minimal mode is active.");

    let _ = rustls::crypto::ring::default_provider().install_default();
    
    // Load Env
    dotenv::dotenv().ok();
    
    // Cancel all orders via HTTP BEFORE starting (Clean Start)
    let api_key = std::env::var("BYBIT_API_KEY").expect("BYBIT_API_KEY not set");
    let api_secret = std::env::var("BYBIT_SECRET_KEY").expect("BYBIT_SECRET_KEY not set");
    
    if let Err(e) = cancel_all_orders_http(&api_key, &api_secret) {
        eprintln!("WARNING: Failed to cancel orders on startup: {}", e);
        eprintln!("Continuing anyway...");
    }

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
            info!("COLD Thread pinned to Core ID: {:?}", cold_core);
        } else {
            eprintln!("WARNING: Failed to pin COLD thread (Windows scheduler restriction?)");
        }

        info!("COLD Thread running.");
        loop {
             while let Ok(msg) = consumer.pop() {
                 if msg.msg_type == 1 || msg.msg_type == 20 { // Status Update OR Quote Adjustment
                     // DISABLED: Too verbose, hiding order logs
                     // But enable for minimal mode if type 20
                     if crate::MINIMAL_LOGS.load(std::sync::atomic::Ordering::Relaxed) {
                         if msg.msg_type == 20 {
                             println!("Lat: {}us", msg.latency);
                         }
                     } else {
                         // let spread_diff = (msg.binance_bid - msg.bybit_ask).max(msg.bybit_bid - msg.binance_ask);
                         // let action_marker = if msg.msg_type == 20 { "[QUOTE]" } else { "       " };
                         // print!("\rBybit: {:.2}/{:.2} | Binance: {:.2}/{:.2} | Diff: {:.2} | Latency: {}us {}   ", 
                         //     msg.bybit_bid, msg.bybit_ask, 
                         //     msg.binance_bid, msg.binance_ask, 
                         //     spread_diff,
                         //     msg.latency,
                         //     action_marker
                         // );
                         // let _ = std::io::stdout().flush();
                     }
                 } else if msg.msg_type == 10 { // Buy Signal
                     info!("\n[SIMULATION] !!! SIGNAL TRIGGERED: BUY (Skewed Quote) !!!");
                 } else if msg.msg_type == 11 { // Sell Signal
                     info!("\n[SIMULATION] !!! SIGNAL TRIGGERED: SELL (Skewed Quote) !!!");
                 }
             }
             thread::sleep(Duration::from_millis(1));
        }
    });

    // HOT THREAD (Strategy)
    let hot_core = core_ids[0]; 
    
    let hot_handle = thread::spawn(move || {
        if core_affinity::set_for_current(hot_core) {
            info!("HOT Thread pinned to Core ID: {:?}", hot_core);
        } else {
            eprintln!("WARNING: Failed to pin HOT thread (Windows scheduler restriction?)");
        }
        
        info!("HOT Thread running.");

        // --- INIT ---
        let mut book = L2OrderBook::new();
        // Strategy is now mutable
        let mut strategy = MarketMaker::new(0.01); 
        let mut risk = RiskEngine::new();
        let mut last_latency = 0; // Track last execution latency
        
        let api_key_env = std::env::var("BYBIT_API_KEY").expect("BYBIT_API_KEY not found in .env");
        let secret_key_env = std::env::var("BYBIT_SECRET_KEY").expect("BYBIT_SECRET_KEY not found in .env");
        
        let api_key = api_key_env.as_str(); // Keep as reference if needed or clone
        let signer = Signer::new(&secret_key_env);

        // --- NETWORK SETUP ---
        info!("HOT: Loading TLS...");
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
        let priv_host = "stream.bybit.com";
        let priv_path = "/v5/private"; // CORRECT V5 Private Endpoint
        let priv_addr = format!("{}:443", priv_host).to_socket_addrs().unwrap().next().unwrap();
        
        let mut ws_private = match WsClient::connect(priv_addr, priv_host, config.clone()) {
            Ok(client) => client,
            Err(e) => {
                eprintln!("CRITICAL ERROR: Failed to connect to Bybit Private Stream: {}", e);
                return;
            }
        };

        // --- TRADE BYBIT SETUP (Orders) ---
        let trade_host = "stream.bybit.com";
        let trade_path = "/v5/trade"; 
        let trade_addr = format!("{}:443", trade_host).to_socket_addrs().unwrap().next().unwrap();
        
        let mut ws_trade = match WsClient::connect(trade_addr, trade_host, config.clone()) {
            Ok(client) => client,
            Err(e) => {
                eprintln!("CRITICAL ERROR: Failed to connect to Bybit Trade: {}", e);
                return;
            }
        };

        // Tokens
        const BYBIT_TOKEN: Token = Token(0);
        const BINANCE_TOKEN: Token = Token(1);
        const BYBIT_PRIVATE_TOKEN: Token = Token(2);
        const BYBIT_TRADE_TOKEN: Token = Token(3);
    
        // Register All
        ws_client.register(poll.registry(), BYBIT_TOKEN).expect("Failed to register Bybit Public");
        ws_binance.register(poll.registry(), BINANCE_TOKEN).expect("Failed to register Binance");
        ws_private.register(poll.registry(), BYBIT_PRIVATE_TOKEN).expect("Failed to register Bybit Private");
        ws_trade.register(poll.registry(), BYBIT_TRADE_TOKEN).expect("Failed to register Bybit Trade");
    
        // Buffers for Binance
        let mut bin_buf = [0u8; 65536];
        let mut bin_offset = 0;
        let mut bin_handshake_done = false;
    
        // Buffers for Bybit
        let mut buf = [0u8; 65536];
        let mut offset = 0; 
    
        // Reuse other buffers
        let mut write_buf = [0u8; 1024]; 
        let mut frame_buf = [0u8; 512]; // Increased for larger order JSON with headers 
        let mut signature_hex = [0u8; 64];
    
        // Buffers for Private
        let mut priv_buf = [0u8; 65536];
        let mut priv_offset = 0;
        
        let mut trade_buf = [0u8; 65536];
        let mut trade_offset = 0;

        let mut tick_count: u64 = 0;
        let mut state = ConnectionState::HandshakeSending;
        let mut priv_state = ConnectionState::HandshakeSending;
        let mut trade_state = ConnectionState::HandshakeSending;
        
        let mut priv_authenticated = false; 
        let mut trade_authenticated = false;
        let mut request_priv_sub = false;        
        let mut bin_active = false;
        
        // Dynamic Time Sync
        // Offset = ServerTime - LocalTime
        let mut time_offset: i64 = 0; 
        let mut offset_initialized = false;
        
        // Auto-Liquidation State
        let mut last_fill_ts: Option<Instant> = None;
    
        info!("HOT: Entering Main Loop (Dual Exchange Mode)...");
        
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
                                info!("HOT: Sending Bybit Handshake...");
                                if let Err(e) = ws_client.send_handshake(host, path) {
                                     eprintln!("Bybit Handshake send error: {}", e);
                                }
                                state = ConnectionState::HandshakeWaiting;
                            }
                            ConnectionState::Subscribing => {
                                let sub_msg = r#"{"op": "subscribe", "args": ["orderbook.50.RIVERUSDT"]}"#;
                                info!("HOT: Sending Bybit Subscription: {}", sub_msg);
                                
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
                                                info!("HOT: Bybit Upgraded! Ready to Subscribe.");
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
                                                         if let Ok(ts) = core::parser::parse_and_update(payload, &mut book) {
                                                                 // Trigger Strategy, but only send if authenticated
                                                             if trade_authenticated {
                                                             let strat_start = Instant::now();
                                                             if let Some(actions) = strategy.on_tick(&book, ts) {
                                                                 let strat_cost = strat_start.elapsed().as_micros();
                                                                 // Loop through actions
                                                                 for action in actions {
                                                                     // Generate timestamp for header
                                                                     // DYNAMIC TIME SYNC
                                                                     let local_now = std::time::SystemTime::now()
                                                                         .duration_since(std::time::UNIX_EPOCH)
                                                                         .unwrap_or_default()
                                                                         .as_millis() as u64;
                                                                     
                                                                     // Apply offset. If offset is negative (Local > Server), we subtract difference.
                                                                     // If not initialized, we try a safe fallback or sending naive time.
                                                                     let ts_ms = if offset_initialized {
                                                                         ((local_now as i64) + time_offset) as u64
                                                                     } else {
                                                                         // Fallback if no response yet: Subtract 2s to be safe
                                                                         local_now.saturating_sub(2000) 
                                                                     };
                                                                     
                                                                     // Send to TRADE WS
                                                                     let req_json = match action.action_type {
                                                                         ActionType::CreateOrder { price, qty, side, link_id } => {
                                                                              info!("HOT: [PERF] CreateOrder generated in {}us", strat_cost);
                                                                              format!(r#"{{"reqId":"{}-{}","header":{{"X-BAPI-TIMESTAMP":"{}","X-BAPI-RECV-WINDOW":"20000"}},"op":"order.create","args":[{{"category":"linear","symbol":"RIVERUSDT","side":"{}","positionIdx":0,"orderType":"Limit","qty":"{:.1}","price":"{:.3}","timeInForce":"PostOnly","orderLinkId":"{}"}}]}}"#, 
                                                                                  link_id, ts_ms, ts_ms, side, qty, price, link_id)
                                                                          },
                                                                         ActionType::AmendOrder { price, qty, side: _, link_id } => {
                                                                             info!("HOT: [PERF] AmendOrder generated in {}us", strat_cost);
                                                                             format!(r#"{{"reqId":"amend-{}-{}","header":{{"X-BAPI-TIMESTAMP":"{}","X-BAPI-RECV-WINDOW":"20000"}},"op":"order.amend","args":[{{"category":"linear","symbol":"RIVERUSDT","qty":"{:.1}","price":"{:.3}","orderLinkId":"{}"}}]}}"#, 
                                                                                 link_id, ts_ms, ts_ms, qty, price, link_id)
                                                                         },
                                                                         ActionType::CancelOrder { link_id } => {
                                                                             info!("HOT: [PERF] CancelOrder generated in {}us", strat_cost);
                                                                             format!(r#"{{"reqId":"cancel-{}-{}","header":{{"X-BAPI-TIMESTAMP":"{}","X-BAPI-RECV-WINDOW":"20000"}},"op":"order.cancel","args":[{{"category":"linear","symbol":"RIVERUSDT","orderLinkId":"{}"}}]}}"#, 
                                                                                 link_id, ts_ms, ts_ms, link_id)
                                                                         },
                                                                         ActionType::ClosePosition { qty, side } => {
                                                                             // Market Order to Close
                                                                             // Use ReduceOnly to prevent flipping position
                                                                             info!("HOT: Strategy requested ClosePosition: Side={}, Qty={} (Calc: {}us)", side, qty, strat_cost);
                                                                             format!(r#"{{"reqId":"close-{}-{}","header":{{"X-BAPI-TIMESTAMP":"{}","X-BAPI-RECV-WINDOW":"20000"}},"op":"order.create","args":[{{"category":"linear","symbol":"RIVERUSDT","side":"{}","positionIdx":0,"orderType":"Market","qty":"{:.1}","timeInForce":"GTC","reduceOnly":true,"orderLinkId":"close-{}-{}"}}]}}"#, 
                                                                                 side, ts_ms, ts_ms, side, qty, side, ts_ms)
                                                                         },
                                                                         ActionType::CancelAll => {
                                                                             info!("HOT: Strategy requested CancelAll (Clean Sweep)");
                                                                             format!(r#"{{"reqId":"cancel-all-{}","header":{{"X-BAPI-TIMESTAMP":"{}","X-BAPI-RECV-WINDOW":"20000"}},"op":"order.cancel-all","args":[{{"category":"linear","symbol":"RIVERUSDT"}}]}}"#, 
                                                                                 ts_ms, ts_ms)
                                                                         },
                                                                         _ => String::new()
                                                                     };
                                                                     
                                                                     let latency = start_tick.elapsed();
                                                                     let lat_u64 = latency.as_micros() as u64;

                                                                     if !req_json.is_empty() {
                                                                         info!(">>> ORDER OUT: {}", req_json);
                                                                         let frame_len = framing::encode_text_frame(req_json.as_bytes(), &mut frame_buf);
                                                                         // Write to TRADE WS
                                                                         // We assume Trade WS is ready. Ideally check state.
                                                                         if frame_len > 0 {
                                                                             if let Err(e) = ws_trade.tls.write_plaintext(&frame_buf[..frame_len]) { 
                                                                                 eprintln!("Order Send Error: {}", e);
                                                                             } else {
                                                                                 // Hack: Force queue write
                                                                                 let _ = ws_trade.write_tls();
                                                                             }
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
                                                             } // end priv_authenticated check
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
                         info!("HOT: Sending Binance Handshake...");
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
                                             info!("HOT: Binance Upgraded!");
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
                                                                 // TRIGGER REMOVED: calling on_tick here mutates state (has_active_... = true)
                                                                 // but we ignore the actions, causing the strategy to think it has an active order
                                                                 // when it doesn't. This causes the 110001 (Order not exists) loop.
                                                                 // strategy.update_binance_price is enough.
                                                                 
                                                                 // Only log status occasionally or just let the Bybit loop handle execution
                                                                 if tick_count % 100 == 0 {
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
                                info!("HOT: Sending Private Handshake...");
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
                                info!("HOT: Authenticating Private WS...");
                                let auth_len = framing::encode_text_frame(auth_msg.as_bytes(), &mut frame_buf);
                                let _ = ws_private.tls.write_plaintext(&frame_buf[..auth_len]);
                                
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
                                info!("HOT: Private WS Read {} bytes, state={:?}", n, priv_state);
                                let end = priv_offset + n;
                                match priv_state {
                                    ConnectionState::HandshakeWaiting => {
                                        if let Ok(s) = std::str::from_utf8(&priv_buf[..end]) {
                                            if s.contains("101 Switching Protocols") {
                                                info!("HOT: Private Switch Proto!");
                                                priv_state = ConnectionState::Authenticating; 
                                                priv_offset = 0;
                                            } else { priv_offset = end; }
                                        }
                                    }
                                    ConnectionState::Active => {
                                        // Parse Executions
                                        // DEBUG: Show raw buffer (hex if not UTF-8)
                                        info!("HOT: Private Active entered, end={}", end);
                                        let _ = std::io::stdout().flush();
                                        let mut current_pos = 0;
                                        loop {
                                            let slice = &mut priv_buf[current_pos..end];
                                            let decode_result = framing::decode_frame(slice);
                                            info!("HOT: decode_frame result: {:?}", decode_result.as_ref().map(|r| r.as_ref().map(|(c, p)| (*c, p.len()))));
                                            match decode_result {
                                                Ok(Some((consumed, payload))) => {
                                                    info!("HOT: Decoded frame, consumed={}, payload_len={}", consumed, payload.len());
                                                    if !payload.is_empty() {
                                                        // LOG ALL PRIVATE RESPONSES
                                                        match std::str::from_utf8(payload) {
                                                            Ok(s) => info!("HOT: Private RAW: {}", s),
                                                            Err(_) => info!("HOT: Private RAW (hex): {:02x?}", &payload[..payload.len().min(50)]),
                                                        }
                                                        if let Ok(json) = simd_json::to_borrowed_value(payload) {                                                             // Check for Error (retCode != 0)
                                                             if let Some(ret_code) = json.get("retCode").and_then(|v| v.as_i64()) {
                                                                 if ret_code != 0 {
                                                                     eprintln!("CRITICAL BYBIT ERROR: {:?}", json);
                                                                     
                                                                     // RECOVERY LOGIC
                                                                     let op = json.get("op").and_then(|v| v.as_str()).unwrap_or("");
                                                                     let ret_msg = json.get("retMsg").and_then(|v| v.as_str()).unwrap_or("");
                                                                     
                                                                     let is_create_fail = op == "order.create";
                                                                     let is_gone = ret_code == 110001; // Order not exists
                                                                     let is_mode_mismatch = ret_code == 10001; // Position mode mismatch OR Params error
                                                                     let is_duplicate = ret_code == 110072; // Duplicate ClOrdID
                                                                     
                                                                     let is_not_modified = ret_msg.contains("not modified");
                                                                     
                                                                     // RECOVERY 1: Reset state on failure (excluding benign "not modified")
                                                                     // We treat DUPLICATE (110072) as a failure that requires RESET (to generate new ID), not restore.
                                                                     if (is_create_fail || is_gone || is_mode_mismatch || is_duplicate) && !is_not_modified {
                                                                         if let Some(req_id) = json.get("reqId").and_then(|v| v.as_str()) {
                                                                              let side_to_reset = if req_id.contains("bot-buy") || req_id.contains("-b-") || req_id.starts_with("b-") { Some("Buy") } 
                                                                                         else if req_id.contains("bot-sell") || req_id.contains("-s-") || req_id.starts_with("s-") { Some("Sell") }
                                                                                         else { None };
                                                                              
                                                                              if let Some(s) = side_to_reset {
                                                                                  eprintln!("HOT: RECOVERY -> Resetting {} state (Code: {})", s, ret_code);
                                                                                  strategy.reset_order(s);
                                                                              }
                                                                         }
                                                                      }
                                                                      
                                                                      // CRITICAL ERROR HANDLING for POSITION LOOP
                                                                      // 110017: ReduceOnly failed because pos is 0
                                                                      // 10404: Params error (often related to invalid qty/price on close)
                                                                      // 10006: Rate Limit (STOP EVERYTHING)
                                                                      if ret_code == 110017 || ret_code == 10404 {
                                                                          eprintln!("STRATEGY: >>> CRITICAL POSITION SYNC ERROR (Code: {}). Forcing Position = 0.", ret_code);
                                                                          strategy.sync_position(0.0, 0.0);
                                                                          strategy.has_active_buy = false;
                                                                          strategy.has_active_sell = false;
                                                                      }
                                                                      if ret_code == 10006 {
                                                                           eprintln!("STRATEGY: >>> API RATE LIMIT EXCEEDED! SLEEPING 10s...");
                                                                           thread::sleep(Duration::from_secs(10));
                                                                      }
                                                                 }
                                                             }
                                                             // Check for Execution
                                                             if let Some(topic) = json.get("topic").and_then(|v| v.as_str()) {
                                                                  if topic == "execution" {
                                                                      // Filled?
                                                                      last_fill_ts = Some(Instant::now());
                                                                      
                                                                      // Parse Execution Data
                                                                      if let Some(data_arr) = json.get("data").and_then(|v| v.as_array()) {
                                                                          for item in data_arr {
                                                                              // Check for Exec Type
                                                                              let exec_type = item.get("execType").and_then(|v| v.as_str()).unwrap_or("");
                                                                              let order_status = item.get("orderStatus").and_then(|v| v.as_str()).unwrap_or("");
                                                                              let side = item.get("side").and_then(|v| v.as_str()).unwrap_or("");
                                                                              
                                                                              if exec_type == "Trade" {
                                                                                   let qty = item.get("execQty").and_then(|v| v.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                                                                                   let px = item.get("execPrice").and_then(|v| v.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                                                                                   println!("\n[EXECUTION] Trade Filled!"); // Always print executions
                                                                                   strategy.on_fill(side, qty, px);
                                                                                   // Use Auto-Liquidation State
                                                                                   last_fill_ts = Some(Instant::now());
                                                                              } else if order_status == "Cancelled" || order_status == "Rejected" || order_status == "Deactivated" {
                                                                                   println!("\n[EXECUTION] Order Cancelled/Rejected! Side: {}", side); // Always print cancellations
                                                                                   strategy.on_order_cancel(side);
                                                                              }
                                                                          }
                                                                      }
                                                                  }
                                                             }
                                                             
                                                             // Check for Position Update (Sync State)
                                                             if let Some(topic) = json.get("topic").and_then(|v| v.as_str()) {
                                                                 if topic == "position" {
                                                                     if let Some(data_arr) = json.get("data").and_then(|v| v.as_array()) {
                                                                         info!("HOT: Received Position Update! Count: {}", data_arr.len());
                                                                         for pos in data_arr {
                                                                             let symbol = pos.get("symbol").and_then(|v| v.as_str()).unwrap_or("UNKNOWN");
                                                                             let side_str = pos.get("side").and_then(|v| v.as_str()).unwrap_or("None");
                                                                             let size_str = pos.get("size").and_then(|v| v.as_str()).unwrap_or("0");
                                                                             let idx = pos.get("positionIdx").and_then(|v| v.as_u64()).unwrap_or(0);
                                                                             
                                                                             info!("HOT: Pos Item -> Sym: {}, Side: {}, Size: {}, Idx: {}", symbol, side_str, size_str, idx);

                                                                             if symbol == "RIVERUSDT" {
                                                                                 let entry_price_str = pos.get("avgPrice").and_then(|v| v.as_str()).unwrap_or("0");
                                                                                 let size = size_str.parse::<f64>().unwrap_or(0.0);
                                                                                 let entry_price = entry_price_str.parse::<f64>().unwrap_or(0.0);
                                                                                 
                                                                                 let signed_qty = if side_str == "Buy" { size } else if side_str == "Sell" { -size } else { 0.0 };
                                                                                 
                                                                                 strategy.sync_position(signed_qty, entry_price);
                                                                             }
                                                                         }
                                                                     }
                                                                 }
                                                             }
                                                             // Check for auth success
                                                             if let Some(op) = json.get("op").and_then(|v| v.as_str()) {
                                                                 if op == "auth" {
                                                                     let is_success = json.get("success").and_then(|v| v.as_bool()).unwrap_or(false) 
                                                                         || json.get("retCode").and_then(|v| v.as_i64()) == Some(0);
                                                                     
                                                                     if is_success {
                                                                         priv_authenticated = true;
                                                                         request_priv_sub = true;
                                                                         info!("HOT: Private WS AUTHENTICATED!");
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

                        if request_priv_sub {
                            request_priv_sub = false;
                            request_priv_sub = false;
                            let exec_sub = r#"{"op": "subscribe", "args": ["execution","position"]}"#;
                            info!("HOT: Sending Private Subscription: {}", exec_sub);
                            let sub_len = framing::encode_text_frame(exec_sub.as_bytes(), &mut frame_buf);
                            let _ = ws_private.tls.write_plaintext(&frame_buf[..sub_len]);
                        }
                    }
                    
                }
                
                BYBIT_TRADE_TOKEN => {
                     if event.is_writable() {
                         match trade_state {
                            ConnectionState::HandshakeSending => {
                                info!("HOT: Sending Trade Handshake...");
                                if let Err(e) = ws_trade.send_handshake(trade_host, trade_path) {
                                     eprintln!("Trade Handshake send error: {}", e);
                                }
                                trade_state = ConnectionState::HandshakeWaiting;
                            }
                            ConnectionState::Authenticating => {
                                // Auth for Trade
                                let expires = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() + 5000;
                                let sign_payload = format!("GET/realtime{}", expires);
                                signer.sign_message(sign_payload.as_bytes(), &mut signature_hex);
                                let sig_str = std::str::from_utf8(&signature_hex[..64]).unwrap_or(""); 
                                
                                let auth_msg = format!(r#"{{"op":"auth","args":["{}","{}","{}"]}}"#, api_key, expires, sig_str);
                                info!("HOT: Authenticating Trade WS...");
                                let auth_len = framing::encode_text_frame(auth_msg.as_bytes(), &mut frame_buf);
                                let _ = ws_trade.tls.write_plaintext(&frame_buf[..auth_len]);
                                
                                trade_state = ConnectionState::Active; 
                            }
                            _ => {}
                        }
                        let _ = ws_trade.write_tls();
                    }

                    if event.is_readable() {
                        if trade_offset >= trade_buf.len() { trade_offset = 0; }
                        match ws_trade.read(&mut trade_buf[trade_offset..]) {
                            Ok(n) if n > 0 => {
                                let end = trade_offset + n;
                                match trade_state {
                                    ConnectionState::HandshakeWaiting => {
                                        if let Ok(s) = std::str::from_utf8(&trade_buf[..end]) {
                                            if s.contains("101 Switching Protocols") {
                                                info!("HOT: Trade Switch Proto!");
                                                trade_state = ConnectionState::Authenticating; 
                                                trade_offset = 0;
                                            } else { trade_offset = end; }
                                        }
                                    }
                                    ConnectionState::Active => {
                                        let mut current_pos = 0;
                                        loop {
                                            let slice = &mut trade_buf[current_pos..end];
                                            let decode_result = framing::decode_frame(slice);
                                            match decode_result {
                                                Ok(Some((consumed, payload))) => {
                                                    if !payload.is_empty() {
                                                        // Clone string BEFORE mutable borrow by simd_json
                                                        let payload_str = std::str::from_utf8(payload).unwrap_or("invalid utf8").to_string();
                                                        info!("HOT: Trade RAW: {}", payload_str);
                                                        
                                                         if let Ok(json) = simd_json::to_borrowed_value(payload) {
                                                             
                                                             // 0. Update Time Offset from Header
                                                             if let Some(header) = json.get("header").and_then(|v| v.as_object()) {
                                                                 if let Some(timenow_val) = header.get("Timenow") {
                                                                      // Timenow can be string or int? Docs show string "167..."
                                                                      let server_time_opt = if let Some(s) = timenow_val.as_str() {
                                                                          s.parse::<u64>().ok()
                                                                      } else {
                                                                           timenow_val.as_u64()
                                                                      };

                                                                      if let Some(server_time) = server_time_opt {
                                                                          let local = std::time::SystemTime::now()
                                                                                .duration_since(std::time::UNIX_EPOCH)
                                                                                .unwrap_or_default()
                                                                                .as_millis() as i64;
                                                                          
                                                                          // Calculate drift
                                                                          // If Server=100, Local=105, Offset = -5.
                                                                          let drift = (server_time as i64) - local;
                                                                          
                                                                          // Smooth update or first set? Let's just set it for now.
                                                                          // But maybe keep the MOST negative drift (furthest back) to be safe?
                                                                          // Actually, simple setting is usually fine for <1 sec latency.
                                                                          // To be safer, we can subtract an extra 500ms from the offset to be "slightly in past"
                                                                          if !offset_initialized {
                                                                               time_offset = drift - 500; 
                                                                               offset_initialized = true;
                                                                               info!("HOT: Time Sync Initialized! Offset: {} ms", time_offset);
                                                                          } else {
                                                                               // Slowly adjust? Or ignore?
                                                                               // Let's ignore subsequent updates to avoid jitter unless huge deviation
                                                                               if (time_offset - drift).abs() > 1000 {
                                                                                    info!("HOT: Time Drift Detected! Old: {}, New: {}. Resyncing.", time_offset, drift);
                                                                                    time_offset = drift - 500;
                                                                               }
                                                                          }
                                                                      }
                                                                 }
                                                             }

                                                             // 1. Check for Auth
                                                             if let Some(op) = json.get("op").and_then(|v| v.as_str()) {
                                                                 if op == "auth" {
                                                                     let is_success = json.get("success").and_then(|v| v.as_bool()).unwrap_or(false) 
                                                                         || json.get("retCode").and_then(|v| v.as_i64()) == Some(0);
                                                                     if is_success {
                                                                         trade_authenticated = true;
                                                                         info!("========================================");
                                                                         info!("HOT: Trade WS AUTHENTICATED!");
                                                                         info!("========================================");
                                                                     }
                                                                 }
                                                             }

                                                             // 2. Check for Trade Errors
                                                             if let Some(ret_code) = json.get("retCode").and_then(|v| v.as_i64()) {
                                                                 if ret_code != 0 {
                                                                      let ret_msg = json.get("retMsg").and_then(|v| v.as_str()).unwrap_or("");
                                                                      println!("HOT: Trade Error Code: {}, Msg: {}", ret_code, ret_msg);
                                                                      
                                                                      // A. Position is Zero (110017) -> Stop Closing Loop
                                                                      if ret_code == 110017 {
                                                                          info!("HOT: Trade -> Position already closed (110017). Syncing to 0.");
                                                                          strategy.sync_position(0.0, 0.0);
                                                                          // Also reset flags just in case
                                                                          strategy.has_active_buy = false;
                                                                          strategy.has_active_sell = false;
                                                                      }
                                                                      // B. Order Not Found (110001) -> Reset Order State
                                                                      else if ret_code == 110001 {
                                                                           if let Some(req_id) = json.get("reqId").and_then(|v| v.as_str()) {
                                                                               let side_to_reset = if req_id.contains("bot-buy") || req_id.contains("-b-") || req_id.starts_with("b-") { Some("Buy") } 
                                                                                          else if req_id.contains("bot-sell") || req_id.contains("-s-") || req_id.starts_with("s-") { Some("Sell") }
                                                                                          else { None };
                                                                               
                                                                               if let Some(s) = side_to_reset {
                                                                                   info!("HOT: Trade -> Order Lost/Late (110001). Resetting {} state.", s);
                                                                                   strategy.reset_order(s);
                                                                               }
                                                                           }
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
                                             trade_buf.copy_within(current_pos..end, 0);
                                             trade_offset = end - current_pos;
                                        } else { trade_offset = 0; }
                                    }
                                    _ => { trade_offset = 0; }
                                }
                            }
                            Ok(_) => {},
                            Err(_) => {},
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

        let trade_interest = if ws_trade.tls.wants_write() || trade_state != ConnectionState::Active {
             mio::Interest::READABLE | mio::Interest::WRITABLE
        } else {
             mio::Interest::READABLE
        };
        poll.registry().reregister(ws_trade.tls.socket(), BYBIT_TRADE_TOKEN, trade_interest).unwrap();

        tick_count = tick_count.wrapping_add(1);
    }
    });

    hot_handle.join().unwrap();
    cold_handle.join().unwrap();
}
