//! quickstart: the smallest believable Toasty app on SurrealDB — a user table
//! taken through one full pass of define → create → read → update → delete.
//!
//! Run it with `cargo run --example quickstart`. It uses the embedded in-memory
//! engine (`kv-mem`), so it needs nothing installed and leaves nothing behind.
//!
//! Unlike the SQL drivers, this driver is attached with
//! `Db::builder().build(SurrealDb::mem())` rather than a connection URL — the
//! SurrealDB driver does not register a URL scheme.

use toasty_driver_surreal::SurrealDb;

// A model is a plain struct. `#[derive(toasty::Model)]` reads the fields and
// attributes to infer the table schema and generate the query/create/update
// methods used below — you never hand-write SurrealQL.
#[derive(Debug, toasty::Model)]
struct User {
    // `#[key]` marks the primary key; it maps to the SurrealDB record id
    // (`user:<id>`). Here we assign ids explicitly.
    #[key]
    id: i64,

    name: String,

    // `#[unique]` enforces "no two users share an email" via a
    // `DEFINE INDEX ... UNIQUE`, and is what makes `get_by_email` /
    // `filter_by_email` exist.
    #[unique]
    email: String,

    age: i64,
}

#[tokio::main]
async fn main() -> toasty::Result<()> {
    // `models!` discovers every `#[derive(Model)]` in this example.
    let mut db = toasty::Db::builder()
        .models(toasty::models!(User))
        .build(SurrealDb::mem())
        .await?;

    // A fresh database has no tables. `push_schema` issues the `DEFINE TABLE`
    // and `DEFINE INDEX` statements straight from the models.
    db.push_schema().await?;
    println!("connected to embedded SurrealDB (kv-mem); schema created\n");

    // --- create ---------------------------------------------------------------
    // `create!` returns a builder; nothing runs until `.exec(&mut db)`. The
    // returned value is fully populated.
    let alice = toasty::create!(User {
        id: 1,
        name: "Alice",
        email: "alice@example.com",
        age: 30,
    })
    .exec(&mut db)
    .await?;
    println!(
        "created {} <{}> (age {})",
        alice.name, alice.email, alice.age
    );

    toasty::create!(User {
        id: 2,
        name: "Bob",
        email: "bob@example.com",
        age: 41,
    })
    .exec(&mut db)
    .await?;

    // --- read -----------------------------------------------------------------
    // By primary key (a SurrealDB `SELECT ... FROM user:1`).
    let found = User::get_by_id(&mut db, 1).await?;
    println!("get_by_id(1) -> {}", found.name);

    // By the unique email index.
    let by_email = User::get_by_email(&mut db, "bob@example.com").await?;
    println!("get_by_email -> {} (age {})", by_email.name, by_email.age);

    // A filtered, ordered scan. `age > 18` has no index, so the driver emits a
    // full-table `SELECT ... WHERE age > 18`.
    let adults: Vec<User> = User::filter(User::fields().age().gt(18))
        .exec(&mut db)
        .await?;
    println!("{} user(s) over 18", adults.len());

    // --- update ---------------------------------------------------------------
    // `update!` on a loaded instance writes only the named field — to the
    // database (`UPDATE user:1 SET age = 31`) and to `alice` in memory.
    let mut alice = found;
    toasty::update!(alice { age: 31 }).exec(&mut db).await?;
    println!("after update, Alice is {}", alice.age);

    // --- delete ---------------------------------------------------------------
    alice.delete().exec(&mut db).await?;
    let gone = User::get_by_id(&mut db, 1).await;
    println!("Alice deleted; get_by_id(1) is_err = {}", gone.is_err());

    Ok(())
}
