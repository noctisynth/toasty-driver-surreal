# toasty-driver-surreal

An out-of-tree [SurrealDB](https://surrealdb.com) driver for the
[Toasty](https://github.com/tokio-rs/toasty) ORM.

SurrealDB is integrated as a **key-value / document backend**. The driver
reports `Capability::sql = None`, so Toasty's query engine emits key-value
operations (`Insert`, `GetByKey`, `QueryPk`, `Scan`, `UpdateByKey`,
`DeleteByKey`, `Upsert`, `FindPkByIndex`), which the driver translates into
SurrealQL and runs against the embedded `surrealdb` SDK. It does **not**
implement a new SQL dialect and does not depend on `toasty-sql`. The DynamoDB
driver is its structural blueprint.

## Status

First-stage implementation plus SurrealKV, explicit transactions, and native
JSON. **609 of 616** generated shared Toasty integration-suite tests pass
directly against the in-memory engine. Seven operation-shape assertions are
excluded for documented upstream-suite reasons, with equivalent driver-owned
runtime coverage (see [Known divergences](#known-divergences)). SurrealKV and
RocksDB end-to-end suites cover file-backed behavior.

## Usage

Attach the driver with `Db::builder().build(driver)` — the driver does not
register a URL scheme, so `.connect(url)` is not used.

```rust
use toasty::Db;
use toasty_driver_surreal::SurrealDb;

#[derive(Debug, toasty::Model)]
struct User {
    #[key]
    id: i64,
    name: String,
}

#[tokio::main]
async fn main() -> toasty::Result<()> {
    // In-memory engine (kv-mem, always available).
    let mut db = Db::builder()
        .models(toasty::models!(User))
        .build(SurrealDb::mem())
        .await?;
    db.push_schema().await?;

    toasty::create!(User { id: 1, name: "Alice" }).exec(&mut db).await?;
    let user = User::get_by_id(&mut db, 1).await?;
    assert_eq!(user.name, "Alice");
    Ok(())
}
```

### Engines

| Constructor | Engine | Availability |
|---|---|---|
| `SurrealDb::mem()` | in-memory (`kv-mem`) | default |
| `SurrealDb::surrealkv(path)` | embedded file (`kv-surrealkv`) | default |
| `SurrealDb::rocksdb(path)` | embedded file (`kv-rocksdb`) | `rocksdb` feature |

SurrealKV is SurrealDB's native file-backed engine and is available without an
extra crate feature. The `rocksdb` feature is off by default because it compiles
`librocksdb` from source. Enable it for the RocksDB engine:

```toml
toasty-driver-surreal = { version = "0.1", features = ["rocksdb"] }
```

Namespace and database default to `"toasty"`; override with
`.namespace(..)` / `.database(..)`.

### Explicit transactions

Top-level explicit transactions work with every embedded engine:

```rust
let mut tx = db.transaction().await?;
toasty::create!(User { id: 2, name: "Bob" })
    .exec(&mut tx)
    .await?;
tx.commit().await?;
```

`transaction_builder().read_only(true)` is supported and rejects writes with
`Error::read_only_transaction`. Explicit isolation levels, non-default
`TransactionMode`s, nested transactions, and savepoints are not supported by
the SurrealDB 3.2.4 client transaction API.

The driver remains a KV/document backend with `Capability::sql = None`.
Explicit `db.transaction()` calls are atomic, but Toasty 0.10 does not
automatically wrap an ordinary `toasty::batch` in a transaction for non-SQL
drivers; callers that need atomicity must execute their operations through an
explicit transaction handle.

### Native JSON

SurrealDB native JSON is available through Toasty's explicit JSON column type:

```rust
#[derive(Debug, toasty::Model)]
struct Event {
    #[key]
    id: i64,
    #[column(type = json)]
    payload: serde_json::Value,
}
```

`toasty::Json<T>` works with the same `#[column(type = json)]` declaration.
Objects, arrays, scalars, and JSON literal `null` are stored as native
SurrealDB values. `Option<Json<T>>::None` remains a database empty value,
distinct from `Json<Option<T>>::None` (JSON `null`). The driver reports
`native_json = true`; `native_jsonb` remains unsupported because SurrealDB has
no separate JSONB storage contract.

## Supported

- CRUD by primary key (single-column `i64` / `String` / `Uuid`, and composite
  keys via SurrealDB array record ids).
- Filtered queries: comparisons, `AND`/`OR`/`NOT`, `IN`, `BETWEEN`, `IS NONE`,
  `starts_with`, array `contains`/superset/intersect/length, and `#[document]`
  nested-field paths.
- Ordering and pagination (offset and keyset cursor).
- Secondary indexes (`DEFINE INDEX`, including `UNIQUE` and composite).
- Native `UPSERT` (create-or-update and insert-or-ignore) with `#[default]` and
  `on_create` semantics.
- Explicit top-level commit/rollback transactions, read-your-writes, read-only
  write protection, and serialization-failure classification for write
  conflicts.
- `#[document]` embedded structs and collections; temporal / decimal values
  (via `toasty-core`'s `jiff` + `rust_decimal`, stored as canonical text).
- Native JSON columns for `toasty::Json<T>` and `serde_json::Value`, including
  database-null versus JSON-null semantics.

## Not supported (first stage)

Remote engines (`ws://`/`http://`), native JSONB, nested transactions/savepoints,
explicit isolation or non-default lock modes, graph edges, live queries, migration
generation, URL-scheme registration, and raw SurrealQL pass-through. Each
requires a spec update before implementation.

## Known divergences

Three shared-suite tests assert *limitations* specific to the DynamoDB
implementation, gated only on `requires(not(sql))`. SurrealDB does not share
those limitations, so the driver correctly performs the operation and the
tests "fail" by succeeding. Per Toasty's philosophy of not hiding backend
differences, the driver does not artificially reject them:

- `composite_index_too_many_range_columns` — SurrealDB indexes any number of
  columns.
- `composite_unique_index_unsupported_on_dynamodb` — SurrealDB enforces
  multi-column `UNIQUE` indexes.
- `starts_with_empty_prefix` — `string::starts_with(x, "")` legitimately
  matches.

Four native-JSON shared tests correctly describe desired data behavior but
hard-code SQL `QuerySql` + typed-parameter log shapes. A KV driver instead
uses inline Insert, GetByKey, and UpdateByKey operations, so those four
operation-shape assertions are skipped; [tests/native_json.rs](tests/native_json.rs)
and the SurrealKV/RocksDB e2e tests cover the same runtime contract.

## Testing

```sh
cargo test                                   # unit + in-memory suite + smoke
cargo test --test transactions -- --test-threads=1
cargo test --test native_json -- --test-threads=1
cargo test --test e2e_surrealkv -- --test-threads=1
cargo test --test e2e_rocksdb --features rocksdb -- --test-threads=1
```

The SurrealKV and RocksDB e2e tests write to `.e2e-data/` (git-ignored).

## Examples

Runnable examples live in `examples/` and use the in-memory engine:

```sh
cargo run --example quickstart          # define -> create -> read -> update -> delete
cargo run --example relationships       # has_many / belongs_to, filter, order, paginate
cargo run --example documents_upsert    # #[document] embeds + native UPSERT
```

## Design docs

Engineering design lives under `.agents/`: accepted
[driver](.agents/rfcs/0001-surrealdb-driver.md),
[SurrealKV](.agents/rfcs/0002-surrealkv-engine.md),
[transaction](.agents/rfcs/0003-explicit-transactions.md), and
[native JSON](.agents/rfcs/0004-native-json.md) RFCs; the
[active spec](.agents/specs/driver.md); and the implementation checklists.
