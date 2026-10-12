//! Point lookup detection for SELECT queries.
//!
//! Detects queries that can be optimized by routing directly to the base table
//! instead of through the UNION view. A point lookup is a SELECT with a WHERE
//! clause that filters on the primary key with equality conditions.

use sqlparser::ast::{BinaryOperator, Expr, Query, Select, SetExpr, Statement, TableFactor, Value};
use std::collections::HashMap;

/// Represents a detected point lookup query.
#[derive(Debug, Clone, PartialEq)]
pub struct PointLookup {
    /// The table being queried (lowercase).
    pub table_name: String,
    /// The primary key column name.
    pub pk_column: String,
    /// The primary key values being looked up.
    pub pk_values: Vec<String>,
}

/// Attempts to detect a point lookup pattern in a SELECT query.
///
/// Returns `Some(PointLookup)` if the query is a simple point lookup that can
/// potentially be routed directly to the base table.
///
/// # Arguments
/// * `stmt` - The parsed SQL statement
/// * `table_pk_map` - Map from table name (lowercase) to primary key column name
///
/// # Returns
/// * `Some(PointLookup)` if this is a point lookup on a known table
/// * `None` if not a point lookup or table PK is unknown
pub fn detect_point_lookup(
    stmt: &Statement,
    table_pk_map: &HashMap<String, String>,
) -> Option<PointLookup> {
    // Must be a Query statement (SELECT)
    let query = match stmt {
        Statement::Query(q) => q,
        _ => return None,
    };

    detect_point_lookup_in_query(query, table_pk_map)
}

/// Detect point lookup in a Query AST node.
pub fn detect_point_lookup_in_query(
    query: &Query,
    table_pk_map: &HashMap<String, String>,
) -> Option<PointLookup> {
    // No CTEs allowed for simple point lookups
    if query.with.is_some() {
        return None;
    }

    // Must be a simple SELECT (no UNION, INTERSECT, etc.)
    let select = match query.body.as_ref() {
        SetExpr::Select(s) => s,
        _ => return None,
    };

    analyze_select(select, table_pk_map)
}

/// Analyze a SELECT statement for point lookup patterns.
fn analyze_select(select: &Select, table_pk_map: &HashMap<String, String>) -> Option<PointLookup> {
    // Must have exactly one table in FROM (no JOINs)
    if select.from.len() != 1 {
        return None;
    }

    let from = &select.from[0];

    // No JOINs allowed
    if !from.joins.is_empty() {
        return None;
    }

    // Extract the table name
    let table_name = extract_table_name(&from.relation)?;
    let table_name_lower = table_name.to_lowercase();

    // Look up the PK column for this table
    let pk_column = table_pk_map.get(&table_name_lower)?;

    // Must have a WHERE clause
    let selection = select.selection.as_ref()?;

    // Try to extract PK values from the WHERE clause
    let pk_values = extract_pk_values(selection, pk_column)?;

    if pk_values.is_empty() {
        return None;
    }

    Some(PointLookup {
        table_name: table_name_lower,
        pk_column: pk_column.clone(),
        pk_values,
    })
}

/// Extract the table name from a TableFactor.
fn extract_table_name(table: &TableFactor) -> Option<String> {
    match table {
        TableFactor::Table { name, .. } => {
            // Get the last identifier (the actual table name, ignoring schema)
            name.0.last().map(|ident| ident.value.clone())
        }
        _ => None, // Subqueries, derived tables, etc. are not point lookups
    }
}

/// Extract primary key values from a WHERE clause.
///
/// Supports:
/// - `pk = value`
/// - `pk IN (value1, value2, ...)`
/// - `pk = value AND other_condition` (extracts pk, ignores other)
///
/// Does NOT support (returns None):
/// - `pk > value` (range)
/// - `pk = value OR other` (OR conditions)
/// - Subqueries
fn extract_pk_values(expr: &Expr, pk_column: &str) -> Option<Vec<String>> {
    match expr {
        // Simple equality: pk = value
        Expr::BinaryOp { left, op, right } => {
            match op {
                BinaryOperator::Eq => {
                    // Check if left side is the PK column
                    if is_column_ref(left, pk_column) {
                        if let Some(val) = extract_literal_value(right) {
                            return Some(vec![val]);
                        }
                    }
                    // Check if right side is the PK column (value = pk)
                    if is_column_ref(right, pk_column) {
                        if let Some(val) = extract_literal_value(left) {
                            return Some(vec![val]);
                        }
                    }
                    None
                }
                BinaryOperator::And => {
                    // For AND, try to extract PK from either side
                    // pk = 1 AND name = 'foo' -> extract pk = 1
                    if let Some(vals) = extract_pk_values(left, pk_column) {
                        return Some(vals);
                    }
                    extract_pk_values(right, pk_column)
                }
                // OR conditions are not point lookups (could expand result set unpredictably)
                BinaryOperator::Or => None,
                // Range conditions are not point lookups
                _ => None,
            }
        }

        // IN list: pk IN (1, 2, 3)
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            // NOT IN is not a point lookup
            if *negated {
                return None;
            }

            // Check if the expression is the PK column
            if !is_column_ref(expr, pk_column) {
                return None;
            }

            // Extract all literal values from the list
            let values: Vec<String> = list.iter().filter_map(extract_literal_value).collect();

            // All values must be literals
            if values.len() != list.len() {
                return None;
            }

            Some(values)
        }

        // Nested expression
        Expr::Nested(inner) => extract_pk_values(inner, pk_column),

        _ => None,
    }
}

/// Check if an expression is a reference to a specific column.
fn is_column_ref(expr: &Expr, column_name: &str) -> bool {
    match expr {
        Expr::Identifier(ident) => ident.value.eq_ignore_ascii_case(column_name),
        Expr::CompoundIdentifier(idents) => {
            // table.column or schema.table.column - check last part
            idents
                .last()
                .map(|i| i.value.eq_ignore_ascii_case(column_name))
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// Extract a literal value as a string from an expression.
fn extract_literal_value(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Value(v) => match v {
            Value::Number(n, _) => Some(n.clone()),
            Value::SingleQuotedString(s) => Some(s.clone()),
            Value::DoubleQuotedString(s) => Some(s.clone()),
            Value::Boolean(b) => Some(b.to_string()),
            Value::Null => None, // NULL is not a valid PK lookup
            _ => None,
        },
        Expr::UnaryOp {
            op: sqlparser::ast::UnaryOperator::Minus,
            expr,
        } => {
            // Handle negative numbers
            if let Expr::Value(Value::Number(n, _)) = expr.as_ref() {
                Some(format!("-{}", n))
            } else {
                None
            }
        }
        // Cast expressions: try to extract the inner value
        Expr::Cast { expr, .. } => extract_literal_value(expr),
        Expr::Nested(inner) => extract_literal_value(inner),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlparser::dialect::PostgreSqlDialect;
    use sqlparser::parser::Parser;

    fn parse_and_detect(sql: &str, pk_map: &HashMap<String, String>) -> Option<PointLookup> {
        let dialect = PostgreSqlDialect {};
        let statements = Parser::parse_sql(&dialect, sql).ok()?;
        if statements.len() != 1 {
            return None;
        }
        detect_point_lookup(&statements[0], pk_map)
    }

    fn make_pk_map() -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("users".to_string(), "id".to_string());
        map.insert("orders".to_string(), "id".to_string());
        map.insert("order_items".to_string(), "id".to_string());
        map
    }

    #[test]
    fn test_simple_equality() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM users WHERE id = 42", &pk_map);

        assert!(result.is_some());
        let lookup = result.unwrap();
        assert_eq!(lookup.table_name, "users");
        assert_eq!(lookup.pk_column, "id");
        assert_eq!(lookup.pk_values, vec!["42"]);
    }

    #[test]
    fn test_string_pk() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM users WHERE id = 'user_123'", &pk_map);

        assert!(result.is_some());
        let lookup = result.unwrap();
        assert_eq!(lookup.pk_values, vec!["user_123"]);
    }

    #[test]
    fn test_in_list() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM users WHERE id IN (1, 2, 3)", &pk_map);

        assert!(result.is_some());
        let lookup = result.unwrap();
        assert_eq!(lookup.pk_values, vec!["1", "2", "3"]);
    }

    #[test]
    fn test_equality_with_and() {
        let pk_map = make_pk_map();
        let result = parse_and_detect(
            "SELECT * FROM users WHERE id = 42 AND status = 'active'",
            &pk_map,
        );

        assert!(result.is_some());
        let lookup = result.unwrap();
        assert_eq!(lookup.pk_values, vec!["42"]);
    }

    #[test]
    fn test_equality_reversed() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM users WHERE 42 = id", &pk_map);

        assert!(result.is_some());
        let lookup = result.unwrap();
        assert_eq!(lookup.pk_values, vec!["42"]);
    }

    #[test]
    fn test_no_where_clause() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM users", &pk_map);
        assert!(result.is_none());
    }

    #[test]
    fn test_non_pk_column() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM users WHERE name = 'Alice'", &pk_map);
        assert!(result.is_none());
    }

    #[test]
    fn test_range_condition() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM users WHERE id > 10", &pk_map);
        assert!(result.is_none());
    }

    #[test]
    fn test_or_condition() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM users WHERE id = 1 OR id = 2", &pk_map);
        assert!(result.is_none());
    }

    #[test]
    fn test_join_not_point_lookup() {
        let pk_map = make_pk_map();
        let result = parse_and_detect(
            "SELECT * FROM users u JOIN orders o ON u.id = o.user_id WHERE u.id = 1",
            &pk_map,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_subquery_not_point_lookup() {
        let pk_map = make_pk_map();
        let result = parse_and_detect(
            "SELECT * FROM users WHERE id IN (SELECT user_id FROM orders)",
            &pk_map,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_unknown_table() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM unknown_table WHERE id = 1", &pk_map);
        assert!(result.is_none());
    }

    #[test]
    fn test_case_insensitive_table() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM USERS WHERE id = 1", &pk_map);

        assert!(result.is_some());
        let lookup = result.unwrap();
        assert_eq!(lookup.table_name, "users");
    }

    #[test]
    fn test_case_insensitive_column() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM users WHERE ID = 1", &pk_map);

        assert!(result.is_some());
    }

    #[test]
    fn test_qualified_table_name() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM public.users WHERE id = 1", &pk_map);

        assert!(result.is_some());
        let lookup = result.unwrap();
        assert_eq!(lookup.table_name, "users");
    }

    #[test]
    fn test_not_in_not_point_lookup() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM users WHERE id NOT IN (1, 2)", &pk_map);
        assert!(result.is_none());
    }

    #[test]
    fn test_negative_number() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT * FROM users WHERE id = -1", &pk_map);

        assert!(result.is_some());
        let lookup = result.unwrap();
        assert_eq!(lookup.pk_values, vec!["-1"]);
    }

    #[test]
    fn test_select_specific_columns() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("SELECT id, name FROM users WHERE id = 42", &pk_map);

        assert!(result.is_some());
        let lookup = result.unwrap();
        assert_eq!(lookup.pk_values, vec!["42"]);
    }

    #[test]
    fn test_insert_not_point_lookup() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("INSERT INTO users (id, name) VALUES (1, 'Alice')", &pk_map);
        assert!(result.is_none());
    }

    #[test]
    fn test_update_not_point_lookup() {
        let pk_map = make_pk_map();
        let result = parse_and_detect("UPDATE users SET name = 'Bob' WHERE id = 1", &pk_map);
        assert!(result.is_none());
    }
}
