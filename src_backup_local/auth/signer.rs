use ring::hmac;
use hex;

pub struct Signer {
    key: hmac::Key,
}

impl Signer {
    pub fn new(secret: &str) -> Self {
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        Self { key }
    }

    /// Signs the payload (timestamp + api_key + recv_window + payload)
    /// Returns hex string in pre-allocated buffer (64 chars)
    /// Bybit: sign = hmac_sha256(timestamp + key + recv_window + payload, secret)
    pub fn sign_request(&self, timestamp: u64, api_key: &str, recv_window: u64, payload: &[u8], out_hex: &mut [u8; 64]) {
        let mut ctx = hmac::Context::with_key(&self.key);
        
        ctx.update(timestamp.to_string().as_bytes());
        ctx.update(api_key.as_bytes());
        ctx.update(recv_window.to_string().as_bytes());
        ctx.update(payload);
        
        let tag = ctx.sign();
        hex::encode_to_slice(tag.as_ref(), out_hex).expect("Hex encoding failed");
    }

    pub fn sign_message(&self, payload: &[u8], out_hex: &mut [u8; 64]) {
        let mut ctx = hmac::Context::with_key(&self.key);
        ctx.update(payload);
        let tag = ctx.sign();
        hex::encode_to_slice(tag.as_ref(), out_hex).expect("Hex encoding failed");
    }
}
