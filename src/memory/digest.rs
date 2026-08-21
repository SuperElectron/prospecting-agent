use crate::memory::client::MemoryItem;

const CHARS_PER_TOKEN: usize = 4;

pub fn digest(items: &[MemoryItem], token_budget: usize) -> String {
    let char_budget = token_budget.saturating_mul(CHARS_PER_TOKEN);
    let mut sorted: Vec<&MemoryItem> = items.iter().collect();
    sorted.sort_by_key(|item| std::cmp::Reverse(item.created_at));
    let mut out = String::new();
    for item in sorted {
        let line = format!("- {}\n", item.content.trim());
        if out.len() + line.len() > char_budget {
            continue;
        }
        out.push_str(&line);
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str, created_at: i64) -> MemoryItem {
        MemoryItem {
            id: format!("id-{created_at}"),
            content: content.into(),
            created_at,
            metadata: None,
        }
    }

    #[test]
    fn newest_memories_come_first() {
        let items = vec![item("old fact", 1), item("new fact", 9)];
        let d = digest(&items, 100);
        let new_pos = d.find("new fact").unwrap();
        let old_pos = d.find("old fact").unwrap();
        assert!(new_pos < old_pos);
    }

    #[test]
    fn budget_bounds_the_output() {
        let items: Vec<MemoryItem> = (0..50)
            .map(|i| item("a somewhat long memory line about the account", i))
            .collect();
        let d = digest(&items, 25);
        assert!(d.len() <= 100);
        assert!(!d.is_empty());
    }

    #[test]
    fn zero_budget_yields_empty_digest() {
        let items = vec![item("fact", 1)];
        assert_eq!(digest(&items, 0), "");
    }

    #[test]
    fn oversized_single_item_is_skipped_not_truncated_mid_line() {
        let items = vec![item(&"x".repeat(500), 2), item("short fact", 1)];
        let d = digest(&items, 30);
        assert_eq!(d, "- short fact");
    }
}
