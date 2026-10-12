//! Tracks dirty (modified/deleted) primary keys per table for a session.
//!
//! Used to optimize point lookups: if a key is known to be clean (unmodified),
//! the query can be routed directly to the base table instead of through the
//! UNION view.

use std::collections::{HashMap, HashSet};

/// Tracks which primary keys have been modified in the current session.
///
/// This enables the proxy to route point lookups on clean keys directly to
/// the base table, avoiding the expensive UNION view.
#[derive(Debug, Default)]
pub struct DirtyKeyTracker {
    /// Map from table name to set of dirty primary key values (as strings).
    /// Using strings handles all PK types uniformly (int, uuid, composite).
    dirty_keys: HashMap<String, HashSet<String>>,

    /// Tables that have had range writes (UPDATE/DELETE without point WHERE).
    /// For these tables, all reads must go through the view.
    fully_dirty_tables: HashSet<String>,
}

#[allow(dead_code)]
impl DirtyKeyTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a specific primary key as dirty for a table.
    pub fn mark_dirty(&mut self, table: &str, pk_value: &str) {
        self.dirty_keys
            .entry(table.to_lowercase())
            .or_default()
            .insert(pk_value.to_string());
    }

    /// Mark a composite primary key as dirty.
    /// Values are joined with a pipe delimiter.
    pub fn mark_dirty_composite(&mut self, table: &str, pk_values: &[&str]) {
        let composite_key = pk_values.join("|");
        self.mark_dirty(table, &composite_key);
    }

    /// Mark an entire table as dirty (for range UPDATE/DELETE).
    /// All subsequent reads must go through the view.
    pub fn mark_table_fully_dirty(&mut self, table: &str) {
        self.fully_dirty_tables.insert(table.to_lowercase());
    }

    /// Check if a key is clean (not modified in this session).
    /// Returns true if the key can be read directly from the base table.
    pub fn is_clean(&self, table: &str, pk_value: &str) -> bool {
        let table_lower = table.to_lowercase();

        // If the entire table is dirty, nothing is clean
        if self.fully_dirty_tables.contains(&table_lower) {
            return false;
        }

        // Check if this specific key is dirty
        if let Some(dirty_set) = self.dirty_keys.get(&table_lower) {
            !dirty_set.contains(pk_value)
        } else {
            // No dirty keys for this table, so this key is clean
            true
        }
    }

    /// Check if a composite key is clean.
    pub fn is_clean_composite(&self, table: &str, pk_values: &[&str]) -> bool {
        let composite_key = pk_values.join("|");
        self.is_clean(table, &composite_key)
    }

    /// Check if the entire table is marked as fully dirty.
    pub fn is_table_fully_dirty(&self, table: &str) -> bool {
        self.fully_dirty_tables.contains(&table.to_lowercase())
    }

    /// Clear all tracking (for session reset).
    pub fn clear(&mut self) {
        self.dirty_keys.clear();
        self.fully_dirty_tables.clear();
    }

    /// Get the count of dirty keys for a table (for metrics/debugging).
    pub fn dirty_key_count(&self, table: &str) -> usize {
        self.dirty_keys
            .get(&table.to_lowercase())
            .map(|s| s.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_tracker_all_clean() {
        let tracker = DirtyKeyTracker::new();
        assert!(tracker.is_clean("users", "1"));
        assert!(tracker.is_clean("users", "42"));
        assert!(tracker.is_clean("orders", "100"));
    }

    #[test]
    fn test_mark_dirty_single_key() {
        let mut tracker = DirtyKeyTracker::new();
        tracker.mark_dirty("users", "42");

        assert!(!tracker.is_clean("users", "42"));
        assert!(tracker.is_clean("users", "1"));
        assert!(tracker.is_clean("users", "100"));
    }

    #[test]
    fn test_mark_dirty_multiple_keys() {
        let mut tracker = DirtyKeyTracker::new();
        tracker.mark_dirty("users", "1");
        tracker.mark_dirty("users", "2");
        tracker.mark_dirty("users", "3");

        assert!(!tracker.is_clean("users", "1"));
        assert!(!tracker.is_clean("users", "2"));
        assert!(!tracker.is_clean("users", "3"));
        assert!(tracker.is_clean("users", "4"));
    }

    #[test]
    fn test_mark_dirty_different_tables() {
        let mut tracker = DirtyKeyTracker::new();
        tracker.mark_dirty("users", "1");
        tracker.mark_dirty("orders", "100");

        assert!(!tracker.is_clean("users", "1"));
        assert!(tracker.is_clean("users", "100"));
        assert!(!tracker.is_clean("orders", "100"));
        assert!(tracker.is_clean("orders", "1"));
    }

    #[test]
    fn test_mark_table_fully_dirty() {
        let mut tracker = DirtyKeyTracker::new();
        tracker.mark_table_fully_dirty("users");

        assert!(!tracker.is_clean("users", "1"));
        assert!(!tracker.is_clean("users", "42"));
        assert!(!tracker.is_clean("users", "999999"));
        assert!(tracker.is_clean("orders", "1")); // other tables still clean
    }

    #[test]
    fn test_composite_key() {
        let mut tracker = DirtyKeyTracker::new();
        tracker.mark_dirty_composite("order_items", &["100", "1"]);

        assert!(!tracker.is_clean_composite("order_items", &["100", "1"]));
        assert!(tracker.is_clean_composite("order_items", &["100", "2"]));
        assert!(tracker.is_clean_composite("order_items", &["101", "1"]));
    }

    #[test]
    fn test_case_insensitive_table_names() {
        let mut tracker = DirtyKeyTracker::new();
        tracker.mark_dirty("Users", "1");
        tracker.mark_table_fully_dirty("ORDERS");

        assert!(!tracker.is_clean("users", "1"));
        assert!(!tracker.is_clean("USERS", "1"));
        assert!(!tracker.is_clean("orders", "42"));
        assert!(!tracker.is_clean("Orders", "42"));
    }

    #[test]
    fn test_clear() {
        let mut tracker = DirtyKeyTracker::new();
        tracker.mark_dirty("users", "1");
        tracker.mark_table_fully_dirty("orders");

        tracker.clear();

        assert!(tracker.is_clean("users", "1"));
        assert!(tracker.is_clean("orders", "42"));
        assert!(!tracker.is_table_fully_dirty("orders"));
    }

    #[test]
    fn test_dirty_key_count() {
        let mut tracker = DirtyKeyTracker::new();
        assert_eq!(tracker.dirty_key_count("users"), 0);

        tracker.mark_dirty("users", "1");
        tracker.mark_dirty("users", "2");
        tracker.mark_dirty("users", "3");

        assert_eq!(tracker.dirty_key_count("users"), 3);
        assert_eq!(tracker.dirty_key_count("orders"), 0);
    }

    #[test]
    fn test_uuid_keys() {
        let mut tracker = DirtyKeyTracker::new();
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        tracker.mark_dirty("users", uuid);

        assert!(!tracker.is_clean("users", uuid));
        assert!(tracker.is_clean("users", "550e8400-e29b-41d4-a716-446655440001"));
    }
}
