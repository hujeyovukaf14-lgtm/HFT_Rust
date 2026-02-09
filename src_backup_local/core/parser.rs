use simd_json;
use crate::core::orderbook::{L2OrderBook, Side};
use simd_json::prelude::*;

// Assuming structure of Bybit public depth delta or snapshot.
// For HFT challenge, we often just look for "b" (bids) and "a" (asks) arrays 
// inside the JSON and iterate them.

pub fn parse_and_update(data: &mut [u8], book: &mut L2OrderBook) -> Result<u64, simd_json::Error> {
    // 1. Parse into Tape (Mutable, in-place)
    let tape = simd_json::to_borrowed_value(data)?;

    // Extract Timestamp (ts)
    let ts = tape.get("ts").and_then(|v| v.as_u64()).unwrap_or(0);

    // 2. Navigate without intermediate structs
    // Bybit structure: { "topic": "...", "data": { "b": [[p, q], ...], "a": [[p, q], ...] } }
    
    // We look for "data" -> "b" and "data" -> "a"
    // Note: get() returns reference to Value
    
    if let Some(data_obj) = tape.get("data") {
        
        // Process Bids
        if let Some(bids) = data_obj.get("b") {
            if let Some(arr) = bids.as_array() {
                 for item in arr {
                     // item is [price_string, qty_string] in Bybit usually
                     // or [price_num, qty_num] depending on API version.
                     // Bybit Linear V5 often sends strings.
                     
                     if let Some(arr_entry) = item.as_array() {
                         if arr_entry.len() >= 2 {
                             let p = arr_entry[0].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                             let q = arr_entry[1].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                             
                             if p > 0.0 {
                                 book.update(Side::Buy, p, q);
                             }
                         }
                     }
                 }
            }
        }
        
        // Process Asks
        if let Some(asks) = data_obj.get("a") {
             if let Some(arr) = asks.as_array() {
                 for item in arr {
                     if let Some(arr_entry) = item.as_array() {
                         if arr_entry.len() >= 2 {
                             let p = arr_entry[0].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                             let q = arr_entry[1].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                             
                             if p > 0.0 {
                                 book.update(Side::Sell, p, q);
                             }
                         }
                     }
                 }
            }
        }
    }
    
    Ok(ts)
}
