use crate::memory::{EntityRef, MemoryClient, MemoryError, digest};

pub(crate) fn truncate_on_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut cut = max_bytes;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    &text[..cut]
}

pub(crate) async fn best_effort_memorize(memory: &MemoryClient, entity: &EntityRef, line: &str) {
    match memory.memorize(entity, line, false).await {
        Ok(()) => {}
        Err(MemoryError::Backend(reason)) => {
            tracing::warn!(entity = %entity, reason, "memory backend rejected line");
        }
        Err(other) => {
            tracing::warn!(entity = %entity, error = %other, "memorize failed");
        }
    }
}

pub(crate) async fn best_effort_digest(
    memory: &MemoryClient,
    entity: &EntityRef,
    query: &str,
    limit: usize,
    token_budget: usize,
) -> String {
    match memory.recall(query, Some(entity), limit).await {
        Ok(items) => digest(&items, token_budget),
        Err(e) => {
            tracing::warn!(entity = %entity, error = %e, "memory recall unavailable");
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_on_boundary;

    #[test]
    fn truncation_respects_utf8_boundaries() {
        let text = "héllo wörld";
        let cut = truncate_on_boundary(text, 2);
        assert_eq!(cut, "h");
        assert!(text.is_char_boundary(cut.len()));
        assert_eq!(truncate_on_boundary("short", 100), "short");
        assert_eq!(truncate_on_boundary("exact", 5), "exact");
    }
}
