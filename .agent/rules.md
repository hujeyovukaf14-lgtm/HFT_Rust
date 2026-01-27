# HFT Rust Strict Ruleset

## 1. Memory Management
* **PROHIBITED in Hot Path:** `String`, `Vec::push`, `Box::new`, `Arc::new`, `format!`, `clone()`.
* **REQUIRED:** Use `arrayvec`, `smallvec`, or pre-allocated `Vec` with `.clear()`.
* **STRINGS:** Use `&str` (slices) from the read buffer. If ownership is needed, copy to a pre-allocated `[u8; N]` buffer on stack.

## 2. Concurrency & Architecture
* **PATTERN:** Single Producer Single Consumer (SPSC) Ring Buffer for IPC.
* **CRATE:** Use `rtrb` or `ringbuf` for sending orders/logs from Hot Thread to Cold Thread.
* **LOCKS:** NO `Mutex` or `RwLock` in the hot path. Use atomic flags if absolutely necessary.
* **THREADING:**
    * **Thread 0 (Hot):** Pinned to core 0. Runs the `mio` Event Loop.
    * **Thread 1 (Cold):** Pinned to core 1. Handles generic I/O (logging to disk, heavy arithmetic offload).

## 3. Network & I/O
* **WS PARSING:** Use `simd-json` for parsing JSON updates. Do not deserialize into full structs if you only need "price" and "qty". Use borrowed deserialization.
* **TCP:** Set `TCP_NODELAY` (disable Nagle) and `TCP_QUICKACK` on all sockets.

## 4. Error Handling
* **NO PANICS:** `.unwrap()` is forbidden in runtime code. Use `if let` or propagate errors to the Cold Thread via ring buffer.
* **RECOVERY:** If WebSocket disconnects, the strategy must enter "Safe Mode" (cancel all) instantly before attempting reconnect.