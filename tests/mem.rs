//! Shared Toasty driver integration suite, run against the in-memory
//! (`kv-mem`) SurrealDB engine.
//!
//! Each `driver()` call constructs a fresh `SurrealDb::mem()`, so every test
//! gets an isolated in-memory database; `delete_table` is therefore a no-op.
//!
//! # Known divergences (7 tests)
//!
//! The shared suite carries a handful of negative tests, gated only on
//! `requires(not(sql))`, that assert DynamoDB-implementation-specific
//! *limitations*. SurrealDB does not share those limitations, so the driver
//! correctly performs the operation and these tests "fail" by succeeding.
//! Making the driver reject them would cripple real capability, which
//! contradicts Toasty's design philosophy of not hiding backend differences.
//! They are intentionally left as documented divergences:
//!
//! * `index_composite::composite_index_too_many_range_columns` — DynamoDB caps
//!   a table at one HASH + one RANGE key; SurrealDB indexes any column count.
//! * `index_composite::composite_unique_index_unsupported_on_dynamodb` —
//!   DynamoDB cannot enforce a multi-column unique constraint; SurrealDB can
//!   (`DEFINE INDEX ... UNIQUE`).
//! * `starts_with::starts_with_empty_prefix` — DynamoDB rejects empty-string
//!   key values; SurrealDB's `string::starts_with(x, "")` legitimately matches.
//! * Four `type_serialize::json*_native_*` tests — their behavior assertions
//!   are valid, but their operation-log assertions unconditionally require a
//!   SQL `QuerySql` + typed-parameter shape. This KV driver uses inline Insert,
//!   GetByKey, and UpdateByKey operations; equivalent behavior is covered by
//!   `tests/native_json.rs`.
//!
//! Every other suite test passes.

use toasty_driver_surreal::SurrealDb;

struct SurrealSetup;

impl SurrealSetup {
    fn new() -> Self {
        SurrealSetup
    }
}

#[async_trait::async_trait]
impl toasty_driver_integration_suite::Setup for SurrealSetup {
    fn driver(&self) -> Box<dyn toasty_core::driver::Driver> {
        Box::new(SurrealDb::mem())
    }

    async fn delete_table(&self, _name: &str) {
        // Each test builds a fresh in-memory database, so there is nothing to
        // clean up between tests.
    }
}

// Generate the shared driver tests. Capability flags mirror the SurrealDB
// profile. The upstream 0.10 native-JSON tests hard-code SQL
// QuerySql/typed-param log shapes, so those four generated tests are skipped
// by the quality command and equivalent KV behavior is covered in
// `tests/native_json.rs`.
toasty_driver_integration_suite::generate_driver_tests!(
    SurrealSetup::new(),
    sql: false,
    returning_from_mutation: false,
    auto_increment: false,
    bigdecimal_implemented: false,
    decimal_arbitrary_precision: false,
    native_decimal: false,
    native_varchar: false,
    native_json: true,
    native_jsonb: false,
    native_ilike: false,
    native_timestamp: false,
    native_date: false,
    native_time: false,
    native_datetime: false,
    native_cidr: false,
    native_inet: false,
    native_macaddr: false,
    native_macaddr8: false,
    native_array: false,
    native_enum: false,
    upsert_unique: false,
    upsert_branch_assignments: false,
    vec_scalar: true,
    unique_list_index: false,
    document_collections: true,
    vec_remove: false,
    vec_pop: false,
    vec_remove_at: false,
    backward_pagination: false,
    test_connection_pool: false,
    transaction_lock_mode: false,
);
