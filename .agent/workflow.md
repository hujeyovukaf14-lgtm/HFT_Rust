# Implementation Workflow

## Phase 1: Skeleton & IPC
1. Initialize Cargo project with dependencies: `mio`, `simd-json`, `rtrb`, `core_affinity`, `socket2`, **`rustls`, `webpki-roots`**.
2. Create the `RingBuffer` channel.
3. Spawn "Cold Thread" (Logger) and pin to Core 1.
4. Spawn "Hot Thread" (Strategy) and pin to Core 0.
5. **TEST:** Measure round-trip time of passing a message from Hot to Cold thread.

## Phase 2: Networking (TLS + Raw)
1. **Implement TLS wrapper (`ClientConnection`) around `mio::TcpStream`.**
2. Implement specific `read_tls` and `write_tls` helpers to handle `WANT_READ`/`WANT_WRITE` non-blocking errors.
3. Integrate manual HTTP Upgrade handshake *inside* the encrypted TLS tunnel.
4. Implement `TCP_NODELAY`.
5. **TEST:** Connect to Bybit Testnet (SSL), perform handshake, receive PING/PONG.

## Phase 3: The Engine
1. Implement `OrderBook` using fixed-size arrays (e.g., top 20 levels only).
2. Implement `simd-json` parsing.
3. **TEST:** Benchmark parsing 10,000 JSON messages. Target < 5 microseconds per msg.

## Phase 4: Strategy & Safety
1. Add the Arbitrage logic.
2. Add "Kill Switch" (if latency > X or loss > Y).
3. **FINAL:** Run full simulation with mock data.