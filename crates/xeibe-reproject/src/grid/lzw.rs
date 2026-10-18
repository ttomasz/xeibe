//! TIFF's LZW decompression (TIFF 6.0, section 13): codes most significant
//! bit first, 9 to 12 bits wide, widening one code early.

const CLEAR: u16 = 256;
const END: u16 = 257;
const FIRST: usize = 258;
const MAX_CODES: usize = 4096;

pub(crate) fn decode(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() * 3);
    // Each table entry: (prefix code, last byte, length); the first 256 are
    // single bytes.
    let mut prefix = vec![0u16; MAX_CODES];
    let mut suffix = vec![0u8; MAX_CODES];
    let mut length = vec![0u16; MAX_CODES];
    for i in 0..256 {
        suffix[i] = i as u8;
        length[i] = 1;
    }
    let mut next = FIRST;
    let mut width = 9u32;
    let mut previous: Option<u16> = None;
    let mut bits: u32 = 0;
    let mut nbits = 0u32;
    let mut bytes = input.iter();
    let mut scratch = Vec::new();
    loop {
        while nbits < width {
            let Some(&b) = bytes.next() else {
                // Ran out without an end code: what was decoded stands.
                return Ok(out);
            };
            bits = (bits << 8) | u32::from(b);
            nbits += 8;
        }
        let code = ((bits >> (nbits - width)) & ((1 << width) - 1)) as u16;
        nbits -= width;
        bits &= (1 << nbits) - 1;
        if code == END {
            return Ok(out);
        }
        if code == CLEAR {
            next = FIRST;
            width = 9;
            previous = None;
            continue;
        }
        let Some(prev) = previous else {
            if code > 255 {
                return Err(format!("LZW: code {code} after a clear"));
            }
            out.push(code as u8);
            previous = Some(code);
            continue;
        };
        let known = usize::from(code) < next;
        if !known && usize::from(code) != next {
            return Err(format!("LZW: code {code} beyond the table ({next})"));
        }
        // The string for `code`, or for `prev` plus its own first byte when
        // `code` is the entry about to be added.
        let source = if known { code } else { prev };
        scratch.clear();
        let mut c = source;
        loop {
            scratch.push(suffix[usize::from(c)]);
            if length[usize::from(c)] == 1 {
                break;
            }
            c = prefix[usize::from(c)];
        }
        scratch.reverse();
        let first = scratch[0];
        out.extend_from_slice(&scratch);
        if !known {
            out.push(first);
        }
        if next < MAX_CODES {
            prefix[next] = prev;
            suffix[next] = first;
            length[next] = length[usize::from(prev)] + 1;
            next += 1;
        }
        // The early change: widen when the next code would need it.
        width = match next + 1 {
            n if n >= 2048 => 12,
            n if n >= 1024 => 11,
            n if n >= 512 => 10,
            _ => 9,
        };
        previous = Some(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode as TIFF does, to test the decoder on its own.
    fn encode(data: &[u8]) -> Vec<u8> {
        use std::collections::HashMap;
        let mut table: HashMap<Vec<u8>, u16> = (0..=255u16).map(|i| (vec![i as u8], i)).collect();
        let mut next = FIRST as u16;
        let mut width = 9u32;
        let mut out = Vec::new();
        let (mut acc, mut n) = (0u64, 0u32);
        let mut emit = |code: u16, width: u32, out: &mut Vec<u8>| {
            acc = (acc << width) | u64::from(code);
            n += width;
            while n >= 8 {
                out.push((acc >> (n - 8)) as u8);
                n -= 8;
            }
        };
        emit(CLEAR, width, &mut out);
        let mut w: Vec<u8> = Vec::new();
        for &b in data {
            let mut wb = w.clone();
            wb.push(b);
            if table.contains_key(&wb) {
                w = wb;
            } else {
                emit(table[&w], width, &mut out);
                table.insert(wb, next);
                next += 1;
                // libtiff widens when `free_ent > maxcode`, one entry
                // after the decoder (which lags one entry behind).
                width = match next as usize {
                    n if n >= 1024 => 11,
                    n if n >= 512 => 10,
                    _ => 9,
                };
                w = vec![b];
            }
        }
        if !w.is_empty() {
            emit(table[&w], width, &mut out);
        }
        emit(END, width, &mut out);
        if n > 0 {
            out.push((acc << (8 - n)) as u8);
        }
        out
    }

    #[test]
    fn round_trips() {
        let data: Vec<u8> = (0..5000u32)
            .map(|i| ((i * 7) % 13 + (i / 100) % 3) as u8)
            .collect();
        assert_eq!(decode(&encode(&data)).unwrap(), data);
        let abab = b"TOBEORNOTTOBEORTOBEORNOT".repeat(40);
        assert_eq!(decode(&encode(&abab)).unwrap(), abab);
    }
}
