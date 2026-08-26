# Changelog

<!-- semifold:release version=0.1.0-alpha.0 -->
## v0.1.0-alpha.0

### New Features

- [`7b3b3b0`](https://github.com/noctisynth/toasty-driver-surreal/commit/7b3b3b0b0890babbe18de4237829fe76f113049b): Add a SurrealDB driver for the Toasty ORM as a key-value / document backend. It translates Toasty's key-value operations into SurrealQL against the embedded surrealdb SDK, covering CRUD by primary key, filtered queries, ordering, offset and keyset pagination, secondary indexes, native UPSERT, embedded documents, and temporal/decimal values.

    Supports the in-memory (kv-mem) engine by default and an embedded RocksDB engine behind the optional rocksdb feature.
<!-- semifold:release:end -->
