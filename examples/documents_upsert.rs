//! documents & upsert: `#[document]` embedded structs (stored as native
//! SurrealDB objects) and the native `UPSERT` create-or-update path.
//!
//! Run it with `cargo run --example documents_upsert`. Uses the embedded
//! in-memory engine.

use toasty_driver_surreal::SurrealDb;

// A `#[derive(Embed)]` type is not a table of its own. As a `#[document]`
// field it is stored as a single nested SurrealDB object, so a filter on
// `profile.city` addresses the nested key directly.
#[derive(Debug, Clone, PartialEq, toasty::Embed)]
struct Profile {
    bio: String,
    city: String,
}

#[derive(Debug, toasty::Model)]
struct Account {
    #[key]
    id: i64,
    name: String,

    // Stored as a nested object column `profile: { bio, city }`.
    #[document]
    profile: Profile,
}

#[tokio::main]
async fn main() -> toasty::Result<()> {
    let mut db = toasty::Db::builder()
        .models(toasty::models!(Account))
        .build(SurrealDb::mem())
        .await?;
    db.push_schema().await?;

    // --- create with an embedded document -------------------------------------
    let account = toasty::create!(Account {
        id: 1,
        name: "Alice",
        profile: Profile {
            bio: "Rustacean".into(),
            city: "Seattle".into(),
        },
    })
    .exec(&mut db)
    .await?;
    println!(
        "created {} in {} — \"{}\"",
        account.name, account.profile.city, account.profile.bio
    );

    // The embed round-trips as a whole.
    let reloaded = Account::get_by_id(&mut db, 1).await?;
    assert_eq!(reloaded.profile, account.profile);

    // --- filter on a nested document field ------------------------------------
    // `profile.city = "Seattle"` compiles to a SurrealQL dotted-path predicate.
    let in_seattle: Vec<Account> =
        Account::filter(Account::fields().profile().city().eq("Seattle"))
            .exec(&mut db)
            .await?;
    println!("{} account(s) in Seattle", in_seattle.len());

    // --- native UPSERT: create-or-update by primary key -----------------------
    // First upsert on a fresh id CREATES the row (SurrealDB `UPSERT account:2`).
    let created = Account::upsert_by_id(2)
        .name("Bob")
        .profile(Profile {
            bio: "Gopher turned Rustacean".into(),
            city: "Portland".into(),
        })
        .exec(&mut db)
        .await?;
    println!("upsert(2) created {}", created.name);

    // A second upsert on the SAME id UPDATES it in place — no duplicate error.
    let updated = Account::upsert_by_id(2)
        .name("Bob R.")
        .profile(Profile {
            bio: "Rustacean".into(),
            city: "Portland".into(),
        })
        .exec(&mut db)
        .await?;
    println!("upsert(2) then updated name to {}", updated.name);

    let count: Vec<Account> = Account::all().exec(&mut db).await?;
    println!("total accounts: {}", count.len());

    Ok(())
}
