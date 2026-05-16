const ESC: u8 = 0x1b;
const PRIMARY_DEVICE_ATTRIBUTES_REPLY: &[u8] = b"\x1b[?1;2c";
const MAX_PENDING_SEQUENCE_BYTES: usize = 128;

#[derive(Debug, Default)]
pub struct PtyOutputFilter {
    pending: Vec<u8>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct FilteredPtyOutput {
    pub display_bytes: Vec<u8>,
    pub reply_bytes: Vec<u8>,
}

impl PtyOutputFilter {
    pub fn filter(&mut self, bytes: &[u8]) -> FilteredPtyOutput {
        let mut input = Vec::with_capacity(self.pending.len() + bytes.len());
        input.append(&mut self.pending);
        input.extend_from_slice(bytes);

        let mut filtered = FilteredPtyOutput::default();
        let mut index = 0;
        while index < input.len() {
            if input[index] != ESC {
                filtered.display_bytes.push(input[index]);
                index += 1;
                continue;
            }

            if index + 1 >= input.len() {
                self.pending.extend_from_slice(&input[index..]);
                break;
            }

            if input[index + 1] != b'[' {
                filtered.display_bytes.push(input[index]);
                index += 1;
                continue;
            }

            let Some(end) = find_csi_end(&input, index + 2) else {
                if input.len() - index > MAX_PENDING_SEQUENCE_BYTES {
                    filtered.display_bytes.push(input[index]);
                    index += 1;
                } else {
                    self.pending.extend_from_slice(&input[index..]);
                    break;
                }
                continue;
            };

            let sequence = &input[index..=end];
            if is_primary_device_attributes_query(sequence) {
                filtered
                    .reply_bytes
                    .extend_from_slice(PRIMARY_DEVICE_ATTRIBUTES_REPLY);
            } else {
                filtered.display_bytes.extend_from_slice(sequence);
            }
            index = end + 1;
        }

        filtered
    }

    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

fn find_csi_end(bytes: &[u8], start: usize) -> Option<usize> {
    bytes[start..]
        .iter()
        .position(|byte| is_csi_final(*byte))
        .map(|offset| start + offset)
}

fn is_csi_final(byte: u8) -> bool {
    (0x40..=0x7e).contains(&byte)
}

fn is_primary_device_attributes_query(sequence: &[u8]) -> bool {
    matches!(sequence, b"\x1b[c" | b"\x1b[0c")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_primary_device_attributes_query() {
        let mut filter = PtyOutputFilter::default();

        let filtered = filter.filter(b"before\x1b[cafter");

        assert_eq!(filtered.display_bytes, b"beforeafter");
        assert_eq!(filtered.reply_bytes, b"\x1b[?1;2c");
    }

    #[test]
    fn filters_zero_parameter_primary_device_attributes_query() {
        let mut filter = PtyOutputFilter::default();

        let filtered = filter.filter(b"\x1b[0c");

        assert!(filtered.display_bytes.is_empty());
        assert_eq!(filtered.reply_bytes, b"\x1b[?1;2c");
    }

    #[test]
    fn filters_split_primary_device_attributes_query() {
        let mut filter = PtyOutputFilter::default();

        let first = filter.filter(b"before\x1b[");
        let second = filter.filter(b"cafter");

        assert_eq!(first.display_bytes, b"before");
        assert!(first.reply_bytes.is_empty());
        assert_eq!(second.display_bytes, b"after");
        assert_eq!(second.reply_bytes, b"\x1b[?1;2c");
    }

    #[test]
    fn preserves_other_csi_sequences() {
        let mut filter = PtyOutputFilter::default();

        let filtered = filter.filter(b"\x1b[31mred");

        assert_eq!(filtered.display_bytes, b"\x1b[31mred");
        assert!(filtered.reply_bytes.is_empty());
    }

    #[test]
    fn finish_flushes_incomplete_escape_sequence() {
        let mut filter = PtyOutputFilter::default();

        let filtered = filter.filter(b"\x1b[");

        assert!(filtered.display_bytes.is_empty());
        assert!(filtered.reply_bytes.is_empty());
        assert_eq!(filter.finish(), b"\x1b[");
    }
}
