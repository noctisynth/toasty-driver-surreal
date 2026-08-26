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

First-stage implementation. **609 of 612** shared Toasty integration-suite
tests pass against the in-memory engine, plus a RocksDB end-to-end suite. The
three remaining suite tests are DynamoDB-specific *negative* assertions that
SurrealDB does not share (see [Known divergences](#known-divergences)).

## Usage

Attach the driver with `Db::builder().build(driver)` — the driver does not
register a URL scheme, so `.connect(url)` is not used.

```rust,no_run
use toasty::Db;
use toasty_driver_surreal::SurrealDb;

#[derive(Debug, toasty::Model)]
struct User {
    #[key]
    id: i64,
    name: String,
}

# async fn run() -> toasty::Result<()> {
// In-memory engine (kv-mem, always available).
let mut db = Db::builder()
    .models(toasty::models!(User))
    .build(SurrealDb::mem())
    .await?;
db.push_schema().await?;

toasty::create!(User { id: 1, name: "Alice" }).exec(&mut db).await?;
let user = User::get_by_id(&mut db, 1).await?;
assert_eq!(user.name, "Alice");
# Ok(())
# }
```

### Engines

| Constructor | Engine | Feature |
|---|---|---|
| `SurrealDb::mem()` | in-memory (`kv-mem`) | default |
| `SurrealDb::rocksdb(path)` | embedded file (`kv-rocksdb`) | `rocksdb` |

The `rocksdb` feature is off by default because it compiles `librocksdb` from
source. Enable it for the file-backed engine:

```toml
toasty-driver-surreal = { version = "0.1", features = ["rocksdb"] }
```

Namespace and database default to `"toasty"`; override with
`.namespace(..)` / `.database(..)`.

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
- `#[document]` embedded structs and collections; temporal / decimal values
  (via `toasty-core`'s `jiff` + `rust_decimal`, stored as canonical text).

## Not supported (first stage)

Remote engines (`ws://`/`http://`), explicit transactions, graph edges, live
queries, migration generation, URL-scheme registration, and raw SurrealQL
pass-through. Each requires a spec update before implementation.

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

## Testing

```sh
cargo test                                   # unit + in-memory suite + smoke
cargo test --test e2e_rocksdb --features rocksdb -- --test-threads=1
```

The RocksDB e2e tests write to `.e2e-data/` (git-ignored).

## Examples

Runnable examples live in `examples/` and use the in-memory engine:

```sh
cargo run --example quickstart          # define -> create -> read -> update -> delete
cargo run --example relationships       # has_many / belongs_to, filter, order, paginate
cargo run --example documents_upsert    # #[document] embeds + native UPSERT
```

## Design docs

Engineering design lives under `.agents/`: the accepted
[RFC 0001](.agents/rfcs/0001-surrealdb-driver.md), the
[active spec](.agents/specs/driver.md), the
[SDK spike](.agents/spikes/surrealdb-sdk-3.2.4.md), and the
[implementation checklist](.agents/todos/driver.md).
