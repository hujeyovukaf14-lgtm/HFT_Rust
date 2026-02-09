use std::time::{Instant, Duration};

// HFT Rules:
// DEV_MODE = true  -> Relaxed Latency Checks (Windows/Test)
// DEV_MODE = false -> Strict HFT Rules (Linux/AWS/Prod) -> Panic on >50us latency

// Set to FALSE only before deploying to AWS Singapore!
const DEV_MODE: bool = true;

const MAX_INTERNAL_LATENCY_MICROS: u128 = 50;
const MAX_NETWORK_LATENCY_MS: u128 = 300;

pub struct RiskEngine {
    pub consecutive_errors: u32,
    pub last_packet_ts: Instant,
}

impl RiskEngine {
    pub fn new() -> Self {
        Self {
            consecutive_errors: 0,
            last_packet_ts: Instant::now(),
        }
    }

    pub fn update_packet_time(&mut self) {
        let now = Instant::now();
        if !DEV_MODE {
             // In real env, check if socket was silent for too long
             let diff = now.duration_since(self.last_packet_ts).as_millis();
             if diff > MAX_NETWORK_LATENCY_MS {
                 // For now just log warning, in prod -> disconnect
                 // eprintln!("RISK: Network Lag detected: {}ms", diff);
             }
        }
        self.last_packet_ts = now;
    }

    pub fn check_internal_latency(&self, start: Instant) {
        let elapsed = start.elapsed().as_micros();
        let limit = if DEV_MODE { 5000 } else { MAX_INTERNAL_LATENCY_MICROS };

        if elapsed > limit {
            eprintln!("RISK CRITICAL: Tick processing took {}us (Limit: {}us)", elapsed, limit);
            // In strict mode -> PANIC to kill the process and strict restart
            // On Windows (Dev Mode), we just log it.
            if !DEV_MODE {
                panic!("Latency violation");
            }
        }
    }
}
