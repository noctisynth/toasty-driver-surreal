//! SurrealDB [`Capability`] profile.
//!
//! Built from the DynamoDB baseline with functional-record-update so that any
//! future `toasty-core` capability field is inherited rather than silently
//! left uninitialized (see `.agents/specs/driver.md` §12). Only the fields that
//! genuinely differ from DynamoDB are overridden.

use toasty_core::driver::Capability;

/// Returns the SurrealDB capability profile.
///
/// SurrealDB is a key-value / document backend (`sql = None`) that supports
/// full-table scans with ordering, `OR` predicates, and primary-key upserts —
/// all wider than DynamoDB's conservative defaults.
pub(crate) fn surrealdb_capability() -> &'static Capability {
    const CAP: Capability = Capability {
        driver_name: "SurrealDB",

        // Key-value / document path: no SQL dialect or placeholder syntax.
        sql: None,
        sql_placeholder: None,

        // SurrealQL supports `SELECT ... FROM table [WHERE ...]`. Scans do
        // not sort: the engine's scan path is used for unindexed reads, and
        // the shared contract treats a non-SQL scan as unordered (ordered
        // reads go through the indexed `QueryPk` path instead).
        scan: true,
        scan_supports_sort: false,

        // `WHERE` freely supports `OR` (no DynamoDB key-condition restriction).
        index_or_predicate: true,

        // SurrealDB has a native `UPSERT` that targets the record id.
        upsert_primary_key: true,

        // SurrealQL `WHERE id != ...` has no DynamoDB-style primary-key
        // predicate restriction.
        primary_key_ne_predicate: true,

        // `#[column(type = json)]` values are decoded from Toasty's JSON-text
        // wire representation into native SurrealDB values. SurrealDB has no
        // distinct JSONB storage contract, so that capability stays false via
        // the DynamoDB baseline.
        native_json: true,

        ..Capability::DYNAMODB
    };

    &CAP
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_is_valid() {
        surrealdb_capability()
            .validate()
            .expect("SurrealDB capability profile must satisfy toasty-core invariants");
    }

    #[test]
    fn capability_is_key_value() {
        let cap = surrealdb_capability();
        assert!(cap.sql.is_none());
        assert!(cap.sql_placeholder.is_none());
        assert!(cap.scan);
        assert!(!cap.scan_supports_sort);
        assert!(cap.index_or_predicate);
        assert!(cap.upsert_primary_key);
        assert!(cap.native_json);
        assert!(!cap.native_jsonb);
    }
}
