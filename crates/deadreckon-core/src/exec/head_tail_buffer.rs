use std::collections::VecDeque;
use std::path::Path;

/// A bounded byte buffer that preserves both the beginning and end of output.
#[derive(Debug, Clone)]
pub struct HeadTailBuffer {
    limit: usize,
    head: Vec<u8>,
    tail: VecDeque<u8>,
    omitted_bytes: u64,
}

impl HeadTailBuffer {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            head: Vec::with_capacity(limit / 2),
            tail: VecDeque::with_capacity(limit.saturating_sub(limit / 2)),
            omitted_bytes: 0,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        let head_limit = self.limit / 2;
        let tail_limit = self.limit.saturating_sub(head_limit);
        let head_room = head_limit.saturating_sub(self.head.len());
        let to_head = head_room.min(chunk.len());
        self.head.extend_from_slice(&chunk[..to_head]);
        let remaining = &chunk[to_head..];

        if remaining.is_empty() {
            return;
        }
        if tail_limit == 0 {
            self.omitted_bytes = self.omitted_bytes.saturating_add(remaining.len() as u64);
            return;
        }

        let overflow = self
            .tail
            .len()
            .saturating_add(remaining.len())
            .saturating_sub(tail_limit);
        for _ in 0..overflow.min(self.tail.len()) {
            let _ = self.tail.pop_front();
        }
        if overflow > 0 {
            self.omitted_bytes = self.omitted_bytes.saturating_add(overflow as u64);
        }

        let retained_start = remaining.len().saturating_sub(tail_limit);
        self.tail.extend(&remaining[retained_start..]);
    }

    pub fn omitted_bytes(&self) -> u64 {
        self.omitted_bytes
    }

    pub fn render(&self, full_copy: Option<&Path>) -> String {
        let head = render_head(&self.head);
        let tail_bytes = self.tail.iter().copied().collect::<Vec<_>>();
        let tail = render_tail(&tail_bytes);
        if self.omitted_bytes == 0 {
            return format!("{head}{tail}");
        }

        let full_copy = full_copy
            .map(|path| format!("; full output: {}", path.display()))
            .unwrap_or_default();
        format!(
            "{head}\n[… {} bytes omitted{full_copy}]\n{tail}",
            self.omitted_bytes
        )
    }
}

fn render_head(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(error) if error.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn render_tail(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    for start in 1..=bytes.len().min(3) {
        if let Ok(text) = std::str::from_utf8(&bytes[start..]) {
            return text.to_string();
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::HeadTailBuffer;
    use std::path::Path;

    #[test]
    fn head_tail_keeps_both_ends_and_counts_omitted() {
        let mut buffer = HeadTailBuffer::new(8);
        buffer.push(b"abcd");
        buffer.push(b"efghijkl");

        assert_eq!(buffer.omitted_bytes(), 4);
        let rendered = buffer.render(None);
        assert!(rendered.starts_with("abcd\n"));
        assert!(rendered.ends_with("\nijkl"));
        assert!(rendered.contains("[… 4 bytes omitted]"));
    }

    #[test]
    fn render_marker_names_full_copy_path() {
        let mut buffer = HeadTailBuffer::new(4);
        buffer.push(b"abcdefgh");

        assert_eq!(
            buffer.render(Some(Path::new("/tmp/deadreckon/full.log"))),
            "ab\n[… 4 bytes omitted; full output: /tmp/deadreckon/full.log]\ngh"
        );
    }

    #[test]
    fn utf8_boundaries_never_split() {
        let mut buffer = HeadTailBuffer::new(7);
        buffer.push("start αβγ end".as_bytes());

        let rendered = buffer.render(None);
        assert!(!rendered.contains('\u{fffd}'));
        assert!(rendered.starts_with("sta"));
        assert!(rendered.ends_with(" end"));
    }

    #[test]
    fn under_limit_output_is_untouched() {
        let mut buffer = HeadTailBuffer::new(64);
        let output = "ordinary output\nwith α complete code point\n";
        buffer.push(output.as_bytes());

        assert_eq!(buffer.omitted_bytes(), 0);
        assert_eq!(buffer.render(None), output);
    }
}
