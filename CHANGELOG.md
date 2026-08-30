# Changelog

<!-- semifold:release version=0.1.0-alpha.1 -->
## v0.1.0-alpha.1

### New Features

- [`f599bb6`](https://github.com/noctisynth/toasty-driver-surreal/commit/f599bb6ba1c58e3d24f4fba177df5a2f207c5807): Support explicit top-level transactions with read-only enforcement and conflict classification.
- [`0a7c874`](https://github.com/noctisynth/toasty-driver-surreal/commit/0a7c87408e0daae0d6f5ed9f2b9d1ebf01d08549): Add transactional migration tracking and SurrealQL generation for safe table, index, and SCHEMALESS column changes, with explicit manual guards for unsafe data transformations.
- [`9b4daa5`](https://github.com/noctisynth/toasty-driver-surreal/commit/9b4daa58298a3245cbb7df114940f2e343eeb1ff): Add native JSON columns with precise database-null and JSON-null semantics across KV operations.
- [`9f0eed4`](https://github.com/noctisynth/toasty-driver-surreal/commit/9f0eed40504e92ab168694bb4f35cca09317295a): Add the embedded SurrealKV engine with persistence and end-to-end coverage.
<!-- semifold:release:end -->

<!-- semifold:release version=0.1.0-alpha.0 -->
## v0.1.0-alpha.0

### New Features

- [`7b3b3b0`](https://github.com/noctisynth/toasty-driver-surreal/commit/7b3b3b0b0890babbe18de4237829fe76f113049b): Add a SurrealDB driver for the Toasty ORM as a key-value / document backend. It translates Toasty's key-value operations into SurrealQL against the embedded surrealdb SDK, covering CRUD by primary key, filtered queries, ordering, offset and keyset pagination, secondary indexes, native UPSERT, embedded documents, and temporal/decimal values.

    Supports the in-memory (kv-mem) engine by default and an embedded RocksDB engine behind the optional rocksdb feature.
<!-- semifold:release:end -->
