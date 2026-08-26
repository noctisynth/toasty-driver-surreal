---
toasty-driver-surreal: "minor:feat"
---

Add a SurrealDB driver for the Toasty ORM as a key-value / document backend. It translates Toasty's key-value operations into SurrealQL against the embedded surrealdb SDK, covering CRUD by primary key, filtered queries, ordering, offset and keyset pagination, secondary indexes, native UPSERT, embedded documents, and temporal/decimal values.

Supports the in-memory (kv-mem) engine by default and an embedded RocksDB engine behind the optional rocksdb feature.
