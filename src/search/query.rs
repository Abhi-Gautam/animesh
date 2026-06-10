/// Normalize a user-entered search query into the durable cache key used for
/// source_search_cache rows.
///
/// This is intentionally conservative: lowercase Unicode, trim edges, and
/// collapse whitespace. Source-specific tokenization belongs inside adapters.
pub(crate) fn normalize_query_key(query: &str) -> String {
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_query_key_by_trimming_lowercasing_and_collapsing_spaces() {
        assert_eq!(normalize_query_key("  Frieren   Beyond "), "frieren beyond");
    }
}
