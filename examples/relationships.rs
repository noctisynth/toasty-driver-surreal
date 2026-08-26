//! relationships: how Toasty models and traverses related data on SurrealDB,
//! plus filtering, ordering, and pagination.
//!
//! Run it with `cargo run --example relationships`. Uses the embedded in-memory
//! engine.
//!
//! The theme: `.await` runs a query, `.get()` reads already-loaded data. Because
//! this is a key-value/document backend, related data is loaded per-parent
//! (`author.posts().exec()`) or preloaded with `.include()`; the driver turns
//! each into record-id lookups.

use toasty_driver_surreal::SurrealDb;

#[derive(Debug, toasty::Model)]
struct Author {
    #[key]
    id: i64,
    name: String,

    // `has_many` adds no column here — the link lives on `Post::author_id`.
    // `Deferred` means "not loaded until you ask"; call `author.posts()`.
    #[has_many]
    posts: toasty::Deferred<Vec<Post>>,
}

#[derive(Debug, toasty::Model)]
struct Post {
    #[key]
    id: i64,

    title: String,
    rank: i64,

    // The foreign key back to the author. `#[index]` keeps "find this author's
    // posts" fast (a `DEFINE INDEX` the driver queries) instead of scanning.
    #[index]
    author_id: i64,

    // `belongs_to` is the other side of `Author::posts`. The foreign key
    // (`author_id`) and referenced key (`Author::id`) follow the naming
    // convention, so both are inferred.
    #[belongs_to]
    author: toasty::Deferred<Author>,
}

#[tokio::main]
async fn main() -> toasty::Result<()> {
    let mut db = toasty::Db::builder()
        .models(toasty::models!(Author, Post))
        .build(SurrealDb::mem())
        .await?;
    db.push_schema().await?;

    // --- create parents and children ------------------------------------------
    let alice = toasty::create!(Author {
        id: 1,
        name: "Alice"
    })
    .exec(&mut db)
    .await?;
    let bob = toasty::create!(Author { id: 2, name: "Bob" })
        .exec(&mut db)
        .await?;

    // Pass the parent by reference and Toasty fills in the `author_id` foreign
    // key from it.
    toasty::create!(Post {
        id: 1,
        title: "Async Rust",
        rank: 2,
        author: &alice
    })
    .exec(&mut db)
    .await?;
    toasty::create!(Post {
        id: 2,
        title: "ORM design",
        rank: 1,
        author: &alice
    })
    .exec(&mut db)
    .await?;
    toasty::create!(Post {
        id: 3,
        title: "Hello",
        rank: 1,
        author: &bob
    })
    .exec(&mut db)
    .await?;

    // --- traverse a relationship both ways ------------------------------------
    // From parent to children: `author.posts()` queries the `author_id` index.
    let alice_posts: Vec<Post> = alice.posts().exec(&mut db).await?;
    println!("Alice has {} post(s)", alice_posts.len());

    // From child back up to the parent.
    let first = &alice_posts[0];
    let owner = first.author().exec(&mut db).await?;
    println!("\"{}\" belongs to {}", first.title, owner.name);

    // --- preloading: defeat N+1 with .include() -------------------------------
    // Looping and calling `author.posts().exec()` per row would be one query per
    // author. `.include()` folds the children into the load; `.get()` then reads
    // them from memory.
    let authors = Author::all()
        .include(Author::fields().posts())
        .exec(&mut db)
        .await?;
    for author in &authors {
        println!("{} wrote {} post(s)", author.name, author.posts.get().len());
    }

    // --- filter + order + paginate --------------------------------------------
    // Scope to Alice's posts by the indexed foreign key, order by `rank`, and
    // walk pages of one.
    let page = Post::filter_by_author_id(1)
        .order_by(Post::fields().rank().asc())
        .paginate(1)
        .exec(&mut db)
        .await?;
    println!("first page of Alice's posts: {} row(s)", page.len());
    if let Some(next) = page.next(&mut db).await? {
        println!("second page of Alice's posts: {} row(s)", next.len());
    }

    Ok(())
}
