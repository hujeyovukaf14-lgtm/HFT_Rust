# Project Architecture: rusty-arb-c8i

## Core Modules
1. **core/:** Optimized data structures (`RingBuffer`, `OrderBook`).
2. **net/:** Low-level network wrapper around `mio`.
    * `ws_client.rs`: Custom WebSocket handshake implementation (avoid heavy deps).
    * `tcp_opt.rs`: Syscalls for socket optimization.
3. **strategy/:** The arbitrage logic.
    * `book_manager.rs`: Updates local L2 orderbook from delta updates.
    * `signal.rs`: Compares Binance Price vs Bybit Price.
4. **ipc/:** Communication between threads.
    * `logger.rs`: Reads from `RingBuffer`, writes to disk (async/batched).

## Data Flow (The "Hot Loop")
1. **Poll:** `mio::Poll` waits for read event on Bybit/Binance socket.
2. **Read:** Read bytes into pre-allocated `[u8; 4096]` buffer.
3. **Parse:** `simd_json::from_slice` parses only `u`, `p`, `q` (update ID, price, qty).
4. **Update:** Mutate local `OrderBook` struct (stack allocated).
5. **Signal:** `if binance_mid_price > bybit_ask + threshold -> TRIGGER`.
6. **Execute:** Format JSON order into pre-allocated buffer -> `socket.write_all()`.
7. **Log:** Push event summary to `RingBuffer` (for Thread 1 to handle).