const ESC: u8 = 0x1b;
const OSC_C1: u8 = 0x9d;
const ST_C1: u8 = 0x9c;
const PRIMARY_DEVICE_ATTRIBUTES_REPLY: &[u8] = b"\x1b[?1;2c";
const MAX_PENDING_SEQUENCE_BYTES: usize = 4096;

#[derive(Debug, Default)]
pub struct PtyOutputFilter {
    pending: Vec<u8>,
    discarding_osc52: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct FilteredPtyOutput {
    pub display_bytes: Vec<u8>,
    pub reply_bytes: Vec<u8>,
    pub blocked_clipboard_writes: usize,
}

impl PtyOutputFilter {
    pub fn filter(&mut self, bytes: &[u8]) -> FilteredPtyOutput {
        let mut input = Vec::with_capacity(self.pending.len() + bytes.len());
        input.append(&mut self.pending);
        input.extend_from_slice(bytes);

        let mut filtered = FilteredPtyOutput::default();
        let mut index = 0;
        while index < input.len() {
            if self.discarding_osc52 {
                let Some(end) = find_osc_end(&input, index) else {
                    if input.last() == Some(&ESC) {
                        self.pending.push(ESC);
                    }
                    break;
                };
                self.discarding_osc52 = false;
                index = end.consumed_end;
                continue;
            }

            if input[index] == OSC_C1 {
                let Some(end) = find_osc_end(&input, index + 1) else {
                    if input.len() - index > MAX_PENDING_SEQUENCE_BYTES {
                        if is_osc52_prefix(&input[index..]) {
                            filtered.blocked_clipboard_writes += 1;
                            self.discarding_osc52 = true;
                            if input.last() == Some(&ESC) {
                                self.pending.push(ESC);
                            }
                            index = input.len();
                        } else {
                            filtered.display_bytes.push(input[index]);
                            index += 1;
                        }
                    } else {
                        self.pending.extend_from_slice(&input[index..]);
                        break;
                    }
                    continue;
                };

                let sequence = &input[index..end.sequence_end];
                if is_osc52_sequence(sequence) {
                    filtered.blocked_clipboard_writes += 1;
                } else {
                    filtered.display_bytes.extend_from_slice(sequence);
                }
                index = end.consumed_end;
                continue;
            }

            if input[index] != ESC {
                filtered.display_bytes.push(input[index]);
                index += 1;
                continue;
            }

            if index + 1 >= input.len() {
                self.pending.extend_from_slice(&input[index..]);
                break;
            }

            if input[index + 1] == b']' {
                let Some(end) = find_osc_end(&input, index + 2) else {
                    if input.len() - index > MAX_PENDING_SEQUENCE_BYTES {
                        if is_osc52_prefix(&input[index..]) {
                            filtered.blocked_clipboard_writes += 1;
                            self.discarding_osc52 = true;
                            if input.last() == Some(&ESC) {
                                self.pending.push(ESC);
                            }
                            index = input.len();
                        } else {
                            filtered.display_bytes.push(input[index]);
                            index += 1;
                        }
                    } else {
                        self.pending.extend_from_slice(&input[index..]);
                        break;
                    }
                    continue;
                };

                let sequence = &input[index..end.sequence_end];
                if is_osc52_sequence(sequence) {
                    filtered.blocked_clipboard_writes += 1;
                } else {
                    filtered.display_bytes.extend_from_slice(sequence);
                }
                index = end.consumed_end;
                continue;
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

    pub fn finish(&mut self) -> FilteredPtyOutput {
        let pending = std::mem::take(&mut self.pending);
        let mut filtered = FilteredPtyOutput::default();
        if self.discarding_osc52 {
            self.discarding_osc52 = false;
        } else if is_osc52_prefix(&pending) {
            filtered.blocked_clipboard_writes = 1;
        } else {
            filtered.display_bytes = pending;
        }
        filtered
    }
}

#[derive(Debug, Clone, Copy)]
struct OscEnd {
    sequence_end: usize,
    consumed_end: usize,
}

fn find_osc_end(bytes: &[u8], start: usize) -> Option<OscEnd> {
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            0x07 => {
                return Some(OscEnd {
                    sequence_end: index + 1,
                    consumed_end: index + 1,
                });
            }
            ST_C1 => {
                return Some(OscEnd {
                    sequence_end: index + 1,
                    consumed_end: index + 1,
                });
            }
            ESC if bytes.get(index + 1) == Some(&b'\\') => {
                return Some(OscEnd {
                    sequence_end: index + 2,
                    consumed_end: index + 2,
                });
            }
            _ => index += 1,
        }
    }
    None
}

fn is_osc52_sequence(sequence: &[u8]) -> bool {
    parse_osc_command(sequence) == Some(b"52")
}

fn is_osc52_prefix(sequence: &[u8]) -> bool {
    let Some(payload) = strip_osc_prefix(sequence) else {
        return false;
    };
    payload.starts_with(b"52") && matches!(payload.get(2), None | Some(b';'))
}

fn parse_osc_command(sequence: &[u8]) -> Option<&[u8]> {
    let payload = strip_osc_prefix(sequence)?;
    let command_end = payload
        .iter()
        .position(|byte| matches!(*byte, b';' | 0x07 | ESC | ST_C1))
        .unwrap_or(payload.len());
    Some(&payload[..command_end])
}

fn strip_osc_prefix(sequence: &[u8]) -> Option<&[u8]> {
    sequence
        .strip_prefix(b"\x1b]")
        .or_else(|| sequence.strip_prefix(&[OSC_C1]))
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
    fn filters_osc52_clipboard_sequence() {
        let mut filter = PtyOutputFilter::default();

        let filtered = filter.filter(b"before\x1b]52;c;SGVsbG8=\x07after");

        assert_eq!(filtered.display_bytes, b"beforeafter");
        assert!(filtered.reply_bytes.is_empty());
        assert_eq!(filtered.blocked_clipboard_writes, 1);
    }

    #[test]
    fn filters_c1_osc52_clipboard_sequence() {
        let mut filter = PtyOutputFilter::default();

        let filtered = filter.filter(b"before\x9d52;c;SGVsbG8=\x9cafter");

        assert_eq!(filtered.display_bytes, b"beforeafter");
        assert!(filtered.reply_bytes.is_empty());
        assert_eq!(filtered.blocked_clipboard_writes, 1);
    }

    #[test]
    fn filters_split_osc52_clipboard_sequence() {
        let mut filter = PtyOutputFilter::default();

        let first = filter.filter(b"before\x1b]52;c;");
        let second = filter.filter(b"SGVsbG8=\x1b\\after");

        assert_eq!(first.display_bytes, b"before");
        assert_eq!(first.blocked_clipboard_writes, 0);
        assert_eq!(second.display_bytes, b"after");
        assert_eq!(second.blocked_clipboard_writes, 1);
    }

    #[test]
    fn preserves_non_clipboard_osc_sequences() {
        let mut filter = PtyOutputFilter::default();

        let sequence = b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07";
        let filtered = filter.filter(sequence);

        assert_eq!(filtered.display_bytes, sequence);
        assert_eq!(filtered.blocked_clipboard_writes, 0);
    }

    #[test]
    fn finish_drops_incomplete_osc52_sequence() {
        let mut filter = PtyOutputFilter::default();

        let filtered = filter.filter(b"\x1b]52;c;SGVsbG8=");

        assert!(filtered.display_bytes.is_empty());
        assert_eq!(filtered.blocked_clipboard_writes, 0);

        let flushed = filter.finish();
        assert!(flushed.display_bytes.is_empty());
        assert_eq!(flushed.blocked_clipboard_writes, 1);
    }

    #[test]
    fn drops_oversized_osc52_until_bel_terminator() {
        let mut filter = PtyOutputFilter::default();
        let mut first = b"\x1b]52;c;".to_vec();
        first.extend(std::iter::repeat(b'A').take(MAX_PENDING_SEQUENCE_BYTES));

        let filtered = filter.filter(&first);

        assert!(filtered.display_bytes.is_empty());
        assert_eq!(filtered.blocked_clipboard_writes, 1);

        let next = filter.filter(b"tail\x07after");

        assert_eq!(next.display_bytes, b"after");
        assert_eq!(next.blocked_clipboard_writes, 0);
    }

    #[test]
    fn drops_oversized_osc52_until_split_st_terminator() {
        let mut filter = PtyOutputFilter::default();
        let mut first = b"\x1b]52;c;".to_vec();
        first.extend(std::iter::repeat(b'A').take(MAX_PENDING_SEQUENCE_BYTES));
        first.push(ESC);

        let filtered = filter.filter(&first);

        assert!(filtered.display_bytes.is_empty());
        assert_eq!(filtered.blocked_clipboard_writes, 1);

        let next = filter.filter(b"\\after");

        assert_eq!(next.display_bytes, b"after");
        assert_eq!(next.blocked_clipboard_writes, 0);
    }

    #[test]
    fn finish_flushes_incomplete_escape_sequence() {
        let mut filter = PtyOutputFilter::default();

        let filtered = filter.filter(b"\x1b[");

        assert!(filtered.display_bytes.is_empty());
        assert!(filtered.reply_bytes.is_empty());
        assert_eq!(filter.finish().display_bytes, b"\x1b[");
    }
}
