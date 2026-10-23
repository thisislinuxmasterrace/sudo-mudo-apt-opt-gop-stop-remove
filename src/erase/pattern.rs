//! the overwrite patterns: the bytes each pass writes, and the fill of a buffer with them.

use rand::RngCore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pattern {
    Zeros,
    Ones,
    Random,
}

impl Pattern {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Pattern::Zeros => "zeros",
            Pattern::Ones => "ones",
            Pattern::Random => "random",
        }
    }
}

/// zeros, then ones, then random data, repeated. the last pass is always random,
/// so the final content is not a recognizable fixed pattern.
pub(crate) fn pattern_for_pass(pass: u32, passes: u32) -> Pattern {
    if pass + 1 == passes {
        return Pattern::Random;
    }
    match pass % 3 {
        0 => Pattern::Zeros,
        1 => Pattern::Ones,
        _ => Pattern::Random,
    }
}

pub(crate) fn fill(buf: &mut [u8], pattern: Pattern) {
    match pattern {
        Pattern::Zeros => buf.fill(0x00),
        Pattern::Ones => buf.fill(0xFF),
        Pattern::Random => rand::rng().fill_bytes(buf),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_sequence_ends_with_random() {
        assert_eq!(pattern_for_pass(0, 1), Pattern::Random);
        assert_eq!(pattern_for_pass(0, 3), Pattern::Zeros);
        assert_eq!(pattern_for_pass(1, 3), Pattern::Ones);
        assert_eq!(pattern_for_pass(2, 3), Pattern::Random);
    }
}
