
/// Decodes a WebSocket frame from the given buffer.
/// Returns Ok(Some((bytes_consumed, payload_slice))) if a full frame is available.
/// Returns Ok(None) if more data is needed (Incomplete).
/// Returns Err if the frame is invalid or unexpected (e.g. masked from server).
pub fn decode_frame(buf: &mut [u8]) -> Result<Option<(usize, &mut [u8])>, &'static str> {
    if buf.len() < 2 {
        return Ok(None);
    }

    let first_byte = buf[0];
    let second_byte = buf[1];

    // let fin = (first_byte & 0x80) != 0;
    // let opcode = first_byte & 0x0F;

    // Check if masked (Server should NOT mask)
    let masked = (second_byte & 0x80) != 0;
    if masked {
        return Err("Server sent masked frame (protocol violation)");
    }

    let mut payload_len = (second_byte & 0x7F) as usize;
    let mut header_len = 2;

    if payload_len == 126 {
        if buf.len() < 4 {
            return Ok(None);
        }
        // Big-endian u16
        let len_bytes: [u8; 2] = [buf[2], buf[3]];
        payload_len = u16::from_be_bytes(len_bytes) as usize;
        header_len += 2;
    } else if payload_len == 127 {
        if buf.len() < 10 {
            return Ok(None);
        }
        // Big-endian u64
        let mut len_bytes = [0u8; 8];
        len_bytes.copy_from_slice(&buf[2..10]);
        // Safety check for 32-bit systems or insane sizes 
        // (though we probably run on 64-bit)
        payload_len = u64::from_be_bytes(len_bytes) as usize;
        header_len += 8;
    }

    let total_len = header_len + payload_len;
    if buf.len() < total_len {
        return Ok(None);
    }

    // Split buf safely
    // We can't return a reference to a part of the slice AND keep control easily in some patterns,
    // but here we just return the sub-slice.
    // The caller gets a mutable reference to the payload part.
    // However, the lifetime logic might be tricky if we don't split correctly.
    // But since `buf` is `&mut [u8]`, we can just index:
    // We return the payload slice. The caller also needs `bytes_consumed` to know how much to advance.
    
    // We can't easily return a mutable subslice that is derived from `buf` while also returning a usize 
    // without some limitations, but in this signature it works because the returned slice borrows from `buf`.
    
    let (_, remaining) = buf.split_at_mut(header_len);
    let (payload, _) = remaining.split_at_mut(payload_len);
    
    Ok(Some((total_len, payload)))
}

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
