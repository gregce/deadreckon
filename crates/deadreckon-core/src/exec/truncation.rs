use super::HeadTailBuffer;

/// A named presentation budget for a specific output consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TruncationPolicy {
    pub limit_bytes: usize,
    pub label: &'static str,
}

impl TruncationPolicy {
    pub const fn ledger() -> Self {
        Self {
            limit_bytes: 16 * 1024,
            label: "ledger",
        }
    }

    pub const fn provider_content() -> Self {
        Self {
            limit_bytes: 256 * 1024,
            label: "provider-content",
        }
    }

    pub const fn pane() -> Self {
        Self {
            limit_bytes: 64 * 1024,
            label: "pane",
        }
    }

    pub fn buffer(self) -> HeadTailBuffer {
        HeadTailBuffer::new(self.limit_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::TruncationPolicy;

    #[test]
    fn named_policies_pin_budgets() {
        assert_eq!(
            TruncationPolicy::ledger(),
            TruncationPolicy {
                limit_bytes: 16 * 1024,
                label: "ledger",
            }
        );
        assert_eq!(
            TruncationPolicy::provider_content(),
            TruncationPolicy {
                limit_bytes: 256 * 1024,
                label: "provider-content",
            }
        );
        assert_eq!(
            TruncationPolicy::pane(),
            TruncationPolicy {
                limit_bytes: 64 * 1024,
                label: "pane",
            }
        );
    }
}
