pub fn encode(value: u64, buf: &mut Vec<u8>) {
    let mut v = value;
    loop {
        if v < 0x80 {
            buf.push(v as u8);
            return;
        }
        buf.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
}

pub fn decode(data: &[u8], pos: &mut usize) -> u64 {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        let byte = data[*pos];
        *pos += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return result;
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(value: u64) {
        let mut buf = Vec::new();
        encode(value, &mut buf);
        let mut pos = 0;
        let decoded = decode(&buf, &mut pos);
        assert_eq!(value, decoded);
        assert_eq!(pos, buf.len());
    }

    #[test]
    fn single_byte() {
        roundtrip(0);
        roundtrip(1);
        roundtrip(127);
    }

    #[test]
    fn two_bytes() {
        roundtrip(128);
        roundtrip(16383);
    }

    #[test]
    fn large_values() {
        roundtrip(16384);
        roundtrip(u32::MAX as u64);
        roundtrip(u64::MAX);
    }

    #[test]
    fn multiple_values() {
        let values = [0u64, 127, 128, 1000, 65535, u32::MAX as u64];
        let mut buf = Vec::new();
        for &v in &values {
            encode(v, &mut buf);
        }
        let mut pos = 0;
        for &v in &values {
            assert_eq!(v, decode(&buf, &mut pos));
        }
    }
}
