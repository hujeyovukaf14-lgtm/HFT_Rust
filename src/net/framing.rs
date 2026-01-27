// use std::io; 
// We don't need rand crate for MVP if we use fixed/rotating mask.
// use rand::Rng; 

/// Encodes a text frame (opcode 0x1) with masking (client -> server requirement).
/// Writes directly to dst_buf to avoid allocation.
/// Returns the number of bytes written.
pub fn encode_text_frame(src_payload: &[u8], dst_buf: &mut [u8]) -> usize {
    let payload_len = src_payload.len();
    let mut offset = 0;

    // 1. Byte 0: FIN (0x80) | Opcode (0x1 = Text)
    dst_buf[offset] = 0x81;
    offset += 1;

    // 2. Byte 1: Mask (0x80) | Length
    // For this MVP we assume payload < 125 bytes for subscription messages.
    // If > 125, we need extended logic (not implemented for simplicity here).
    if payload_len < 126 {
        dst_buf[offset] = 0x80 | (payload_len as u8);
        offset += 1;
    } else {
        // Panic or handle error in real code. For now assuming short JSON.
        // eprintln!("Frame too large for simple encoder");
        return 0;
    }

    // 3. Mask Key (4 bytes)
    // Simple rotating mask: [1, 2, 3, 4]
    // In prod use random: rand::random::<[u8; 4]>()
    let mask_key = [1u8, 2, 3, 4];
    dst_buf[offset] = mask_key[0];
    dst_buf[offset+1] = mask_key[1];
    dst_buf[offset+2] = mask_key[2];
    dst_buf[offset+3] = mask_key[3];
    offset += 4;

    // 4. Payload (Masked)
    for (i, &byte) in src_payload.iter().enumerate() {
        dst_buf[offset + i] = byte ^ mask_key[i % 4];
    }
    offset += payload_len;

    offset
}
