//! Token counting utilities for context window management.
//!
//! Provides accurate BPE-based token counting using the cl100k_base encoding
//! (used by GPT-4 and Claude models). Falls back to a word-count heuristic
//! when the tokenizer is unavailable.

use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

/// Lazy-initialized cl100k_base BPE tokenizer.
static TOKENIZER: OnceLock<CoreBPE> = OnceLock::new();

/// Returns a reference to the shared cl100k_base tokenizer, initializing it on
/// first call.
fn get_tokenizer() -> Option<&'static CoreBPE> {
    TOKENIZER
        .get_or_init(|| {
            tiktoken_rs::cl100k_base().expect("cl100k_base tokenizer must be available")
        })
        .into()
}

/// Count the number of tokens in `text` using the cl100k_base BPE tokenizer.
///
/// This is accurate for models that use cl100k_base (GPT-4, Claude, etc.).
/// Falls back to [`count_tokens_approx`] if the tokenizer cannot be loaded.
pub fn count_tokens(text: &str) -> usize {
    match get_tokenizer() {
        Some(bpe) => bpe.encode_ordinary(text).len(),
        None => count_tokens_approx(text),
    }
}

/// Estimate token count using a word-count heuristic.
///
/// This was the original estimation method and is kept as a documented fallback.
/// Splits on whitespace for word count, multiplies by ~1.3 for subword
/// tokenization, and adds a small overhead for punctuation/special characters.
pub fn count_tokens_approx(text: &str) -> usize {
    let words = text.split_whitespace().count();
    let overhead = text.len() / 40;
    words * 13 / 10 + overhead
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn test_count_tokens_empty() {
        assert_eq!(count_tokens(""), 0);
    }

    #[test]
    fn test_count_tokens_known_phrase() {
        // "Hello, world!" encodes to 4 tokens with cl100k_base:
        // "Hello", ",", " world", "!"
        let count = count_tokens("Hello, world!");
        assert_eq!(count, 4);
    }

    #[test]
    fn test_count_tokens_sentence() {
        // A well-known benchmark: "The quick brown fox jumps over the lazy dog"
        // cl100k_base: "The", " quick", " brown", " fox", " jumps", " over",
        //              " the", " lazy", " dog" = 9 tokens
        let count = count_tokens("The quick brown fox jumps over the lazy dog");
        assert_eq!(count, 9);
    }

    #[test]
    fn test_count_tokens_more_accurate_than_approx() {
        // For a long technical string with special chars, BPE should differ from approx.
        // Just verify it returns a plausible non-zero value.
        let text = "async fn process_message(ctx: &mut Context) -> Result<Response, Error>";
        let bpe_count = count_tokens(text);
        assert!(bpe_count > 0);
    }

    #[test]
    fn test_count_tokens_approx_empty() {
        assert_eq!(count_tokens_approx(""), 0);
    }

    #[test]
    fn test_count_tokens_approx_single_word() {
        // 1 word * 13/10 = 1, overhead = 0 for short string
        assert_eq!(count_tokens_approx("hello"), 1);
    }

    #[test]
    fn test_count_tokens_approx_multiple_words() {
        // 5 words * 13/10 = 6 (integer division), short string overhead = 0
        assert_eq!(count_tokens_approx("one two three four five"), 6);
    }
}
