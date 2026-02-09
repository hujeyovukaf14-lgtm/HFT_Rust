use std::io::Write;
use ryu; // Fast float to string

/// Serializes a JSON limit order into the buffer for Bybit V5.
/// Format: {"category":"linear","symbol":"...","side":"...","orderType":"Limit","qty":"...","price":"...","timeInForce":"PostOnly"}
/// Returns the number of bytes written.
pub fn write_order_json(buf: &mut [u8], symbol: &str, side: &str, qty: f64, price: f64) -> usize {
    let mut cursor = std::io::Cursor::new(buf);
    
    // We construct the JSON manually to avoid allocation
    // {"reqId":"...","category":"linear","symbol":"...","side":"...","orderType":"Limit","qty":"...","price":"...","timeInForce":"PostOnly"}
    // For HFT challenge we omit reqId for simplicity unless needed for matching
    
    let _ = write!(cursor, r#"{{"category":"linear","symbol":"{}","side":"{}","orderType":"Limit","qty":""#, symbol, side);
    
    // Write Float Qty
    let mut buffer = ryu::Buffer::new();
    let _ = cursor.write_all(buffer.format(qty).as_bytes());
    
    let _ = cursor.write_all(r#"","price":""#.as_bytes());
    
    // Write Float Price
    let mut buffer2 = ryu::Buffer::new();
    let _ = cursor.write_all(buffer2.format(price).as_bytes());
    
    let _ = cursor.write_all(r#"","timeInForce":"PostOnly"}}"#.as_bytes());
    
    cursor.position() as usize
}
