//! Toasty schema-diff to SurrealQL migration rendering.

use toasty_core::schema::{db, diff};

use crate::expr::escape_ident;

pub(crate) const TRACKING_TABLE: &str = "__toasty_migrations";
pub(crate) const TRACKING_ID_ALIAS: &str = "migration_id";

/// Generates a SurrealQL migration for the subset of schema changes that can
/// be expressed safely for SCHEMALESS tables.
pub(crate) fn generate(schema_diff: &diff::Schema<'_>) -> db::Migration {
    let mut statements = Vec::new();

    for table_diff in schema_diff.tables() {
        #[allow(unreachable_patterns)]
        match table_diff {
            diff::Table::Create(table) => {
                statements.extend(define_table_statements(table, false));
            }
            diff::Table::Drop(table) => {
                statements.push(format!("REMOVE TABLE {}", escape_ident(&table.name)));
            }
            diff::Table::Alter {
                previous,
                next,
                columns,
                indices,
            } => {
                render_alter_table(previous, next, columns, indices, &mut statements);
            }
            _ => push_unique(&mut statements, manual_guard("unknown table schema change")),
        }
    }

    if statements.is_empty() {
        // A SCHEMALESS column addition or metadata-only change has no physical
        // database definition, but Toasty still needs an executable migration
        // file that can be tracked as applied.
        statements.push("RETURN NONE".to_string());
    }

    db::Migration::new_sql_with_breakpoints(&statements)
}

/// Renders the table and all secondary indices. `push_schema` uses idempotent
/// definitions while generated migrations intentionally detect schema drift.
pub(crate) fn define_table_statements(table: &db::Table, if_not_exists: bool) -> Vec<String> {
    let condition = if if_not_exists { " IF NOT EXISTS" } else { "" };
    let mut statements = vec![format!(
        "DEFINE TABLE{condition} {} SCHEMALESS",
        escape_ident(&table.name)
    )];

    for index in &table.indices {
        if !index.primary_key {
            statements.push(define_index_statement(table, index, if_not_exists));
        }
    }

    statements
}

fn render_alter_table(
    previous: &db::Table,
    next: &db::Table,
    columns: &[diff::Column<'_>],
    indices: &[diff::Index<'_>],
    statements: &mut Vec<String>,
) {
    if previous.name != next.name {
        push_unique(
            statements,
            manual_guard(&format!(
                "table rename from {} to {}",
                previous.name, next.name
            )),
        );
        // A table name is part of every SurrealDB record id. Until the guard
        // is replaced with an application-specific record-id migration, no
        // later table-local statement has a valid physical target.
        return;
    }

    let mut guards = Vec::new();
    let mut index_removals = Vec::new();
    let mut data_changes = Vec::new();
    let mut index_definitions = Vec::new();
    let mut renamed_columns = Vec::new();

    if primary_key_changed(previous, next, columns) {
        push_unique(
            &mut guards,
            manual_guard(&format!(
                "primary-key layout or type change on {}",
                next.name
            )),
        );
    }

    for column_diff in columns {
        #[allow(unreachable_patterns)]
        match column_diff {
            diff::Column::Add(_) => {
                // SCHEMALESS tables require no physical field definition.
            }
            diff::Column::Drop(column) => {
                if !is_primary_key(previous, column) {
                    data_changes.push(format!(
                        "UPDATE {} UNSET {} RETURN NONE",
                        escape_ident(&next.name),
                        escape_ident(&column.name)
                    ));
                }
            }
            diff::Column::Alter {
                previous: previous_column,
                next: next_column,
            } => {
                if previous_column.ty != next_column.ty
                    || previous_column.storage_ty != next_column.storage_ty
                {
                    push_unique(
                        &mut guards,
                        manual_guard(&format!(
                            "column type change on {}.{}",
                            next.name, next_column.name
                        )),
                    );
                }

                if previous_column.name != next_column.name {
                    let previous_is_pk = is_primary_key(previous, previous_column);
                    let next_is_pk = is_primary_key(next, next_column);
                    if !previous_is_pk && !next_is_pk {
                        renamed_columns.push((*previous_column, *next_column));
                        data_changes.push(format!(
                            "UPDATE {table} SET {next_column} = {previous_column} \
                             WHERE {previous_column} IS NOT NONE RETURN NONE",
                            table = escape_ident(&next.name),
                            previous_column = escape_ident(&previous_column.name),
                            next_column = escape_ident(&next_column.name),
                        ));
                        data_changes.push(format!(
                            "UPDATE {} UNSET {} RETURN NONE",
                            escape_ident(&next.name),
                            escape_ident(&previous_column.name)
                        ));
                    }
                    // A primary-key column is represented only by record::id;
                    // renaming its Toasty alias needs no physical data rewrite.
                }
            }
            _ => push_unique(
                &mut guards,
                manual_guard(&format!("unknown column change on {}", next.name)),
            ),
        }
    }

    for index_diff in indices {
        #[allow(unreachable_patterns)]
        match index_diff {
            diff::Index::Create(index) => {
                if !index.primary_key {
                    push_unique(
                        &mut index_definitions,
                        define_index_statement(next, index, false),
                    );
                }
            }
            diff::Index::Drop(index) => {
                if !index.primary_key {
                    push_unique(&mut index_removals, remove_index_statement(previous, index));
                }
            }
            diff::Index::Alter {
                previous: previous_index,
                next: next_index,
            } => {
                if !previous_index.primary_key {
                    push_unique(
                        &mut index_removals,
                        remove_index_statement(previous, previous_index),
                    );
                }
                if !next_index.primary_key {
                    push_unique(
                        &mut index_definitions,
                        define_index_statement(next, next_index, false),
                    );
                }
            }
            _ => push_unique(
                &mut guards,
                manual_guard(&format!("unknown index change on {}", next.name)),
            ),
        }
    }

    // Rename hints can make Toasty's index diff consider an index unchanged,
    // but SurrealDB stores physical field names in the index definition. Any
    // secondary index touching a renamed field must therefore be rebuilt.
    for (previous_column, next_column) in renamed_columns {
        for index in &previous.indices {
            if !index.primary_key
                && index
                    .columns
                    .iter()
                    .any(|column| column.column == previous_column.id)
            {
                push_unique(&mut index_removals, remove_index_statement(previous, index));
            }
        }
        for index in &next.indices {
            if !index.primary_key
                && index
                    .columns
                    .iter()
                    .any(|column| column.column == next_column.id)
            {
                push_unique(
                    &mut index_definitions,
                    define_index_statement(next, index, false),
                );
            }
        }
    }

    // Guards come first so an unedited unsafe migration cannot perform even a
    // transient destructive change before the SDK transaction is cancelled.
    statements.extend(guards);
    statements.extend(index_removals);
    // Define replacement indices before moving SCHEMALESS field data. The
    // SurrealDB 3.2.4 index builder cannot see uncommitted values written
    // earlier in this same transaction; subsequent DML does maintain an index
    // that is already defined.
    statements.extend(index_definitions);
    statements.extend(data_changes);
}

fn define_index_statement(table: &db::Table, index: &db::Index, if_not_exists: bool) -> String {
    let condition = if if_not_exists { " IF NOT EXISTS" } else { "" };
    let columns = index
        .columns
        .iter()
        .map(|index_column| escape_ident(&table.column(index_column.column).name))
        .collect::<Vec<_>>()
        .join(", ");
    let unique = if index.unique { " UNIQUE" } else { "" };

    format!(
        "DEFINE INDEX{condition} {index} ON TABLE {table} COLUMNS {columns}{unique}",
        index = escape_ident(&index.name),
        table = escape_ident(&table.name),
    )
}

fn remove_index_statement(table: &db::Table, index: &db::Index) -> String {
    format!(
        "REMOVE INDEX {} ON TABLE {}",
        escape_ident(&index.name),
        escape_ident(&table.name)
    )
}

fn primary_key_changed(
    previous: &db::Table,
    next: &db::Table,
    columns: &[diff::Column<'_>],
) -> bool {
    if previous.primary_key.columns.len() != next.primary_key.columns.len() {
        return true;
    }

    previous
        .primary_key_columns()
        .zip(next.primary_key_columns())
        .any(|(previous_column, next_pk_column)| {
            let mapped = columns.iter().find_map(|change| match change {
                diff::Column::Alter {
                    previous: changed_previous,
                    next: changed_next,
                } if changed_previous.id == previous_column.id => Some(*changed_next),
                _ => None,
            });
            let mapped = mapped.or_else(|| {
                next.columns
                    .iter()
                    .find(|candidate| candidate.name == previous_column.name)
            });

            let Some(mapped) = mapped else {
                return true;
            };
            mapped.id != next_pk_column.id
                || previous_column.ty != mapped.ty
                || previous_column.storage_ty != mapped.storage_ty
        })
}

fn is_primary_key(table: &db::Table, column: &db::Column) -> bool {
    table.primary_key.columns.contains(&column.id)
}

fn manual_guard(reason: &str) -> String {
    let reason = reason.replace('\\', "\\\\").replace('\'', "\\'");
    format!("THROW 'manual migration required: {reason}'")
}

fn push_unique(statements: &mut Vec<String>, statement: String) {
    if !statements.contains(&statement) {
        statements.push(statement);
    }
}

#[cfg(test)]
mod tests {
    use toasty_core::schema::{db, diff};
    use toasty_core::stmt;

    use super::generate;

    fn schema(table_name: &str, value_name: &str) -> db::Schema {
        let table_id = db::TableId(0);
        let id = db::ColumnId {
            table: table_id,
            index: 0,
        };
        let value = db::ColumnId {
            table: table_id,
            index: 1,
        };

        db::Schema {
            tables: vec![db::Table {
                id: table_id,
                name: table_name.to_string(),
                columns: vec![
                    db::Column {
                        id,
                        name: "id".to_string(),
                        ty: stmt::Type::I64,
                        storage_ty: db::Type::Integer(8),
                        nullable: false,
                        primary_key: true,
                        auto_increment: false,
                        versionable: false,
                    },
                    db::Column {
                        id: value,
                        name: value_name.to_string(),
                        ty: stmt::Type::String,
                        storage_ty: db::Type::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        versionable: false,
                    },
                ],
                primary_key: db::PrimaryKey {
                    columns: vec![id],
                    index: db::IndexId {
                        table: table_id,
                        index: 0,
                    },
                },
                indices: vec![
                    db::Index {
                        id: db::IndexId {
                            table: table_id,
                            index: 0,
                        },
                        name: "items_pk".to_string(),
                        on: table_id,
                        columns: vec![db::IndexColumn {
                            column: id,
                            op: db::IndexOp::Eq,
                            scope: db::IndexScope::Local,
                        }],
                        unique: true,
                        primary_key: true,
                    },
                    db::Index {
                        id: db::IndexId {
                            table: table_id,
                            index: 1,
                        },
                        name: "items_value".to_string(),
                        on: table_id,
                        columns: vec![db::IndexColumn {
                            column: value,
                            op: db::IndexOp::Eq,
                            scope: db::IndexScope::Local,
                        }],
                        unique: false,
                        primary_key: false,
                    },
                ],
            }],
        }
    }

    fn statements(
        previous: &db::Schema,
        next: &db::Schema,
        hints: &diff::RenameHints,
    ) -> Vec<String> {
        let schema_diff = diff::Schema::from(previous, next, hints);
        generate(&schema_diff)
            .statements()
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn creates_and_drops_tables_with_secondary_indices() {
        let empty = db::Schema::default();
        let items = schema("items", "value");
        let hints = diff::RenameHints::new();

        let create = statements(&empty, &items, &hints);
        assert_eq!(
            create,
            vec![
                "DEFINE TABLE `items` SCHEMALESS",
                "DEFINE INDEX `items_value` ON TABLE `items` COLUMNS `value`",
            ]
        );
        assert!(
            create
                .iter()
                .all(|statement| !statement.contains("items_pk"))
        );

        assert_eq!(
            statements(&items, &empty, &hints),
            vec!["REMOVE TABLE `items`"]
        );
    }

    #[test]
    fn alters_secondary_index_by_removing_then_defining() {
        let previous = schema("items", "value");
        let mut next = previous.clone();
        next.tables[0].indices[1].unique = true;
        let statements = statements(&previous, &next, &diff::RenameHints::new());

        assert_eq!(
            statements,
            vec![
                "REMOVE INDEX `items_value` ON TABLE `items`",
                "DEFINE INDEX `items_value` ON TABLE `items` COLUMNS `value` UNIQUE",
            ]
        );
    }

    #[test]
    fn schemaless_add_is_noop_and_drop_unsets_stored_data() {
        let full = schema("items", "value");
        let mut id_only = full.clone();
        id_only.tables[0].columns.pop();
        id_only.tables[0].indices.pop();
        let hints = diff::RenameHints::new();

        assert_eq!(
            statements(&id_only, &full, &hints),
            vec!["DEFINE INDEX `items_value` ON TABLE `items` COLUMNS `value`"]
        );
        assert_eq!(
            statements(&full, &id_only, &hints),
            vec![
                "REMOVE INDEX `items_value` ON TABLE `items`",
                "UPDATE `items` UNSET `value` RETURN NONE",
            ]
        );

        let mut no_index_full = full.clone();
        no_index_full.tables[0].indices.pop();
        assert_eq!(
            statements(&id_only, &no_index_full, &hints),
            vec!["RETURN NONE"]
        );
    }

    #[test]
    fn column_rename_moves_data_and_rebuilds_unchanged_index() {
        let previous = schema("items", "value");
        let next = schema("items", "renamed");
        let mut hints = diff::RenameHints::new();
        hints.add_column_hint(
            previous.tables[0].columns[1].id,
            next.tables[0].columns[1].id,
        );

        assert_eq!(
            statements(&previous, &next, &hints),
            vec![
                "REMOVE INDEX `items_value` ON TABLE `items`",
                "DEFINE INDEX `items_value` ON TABLE `items` COLUMNS `renamed`",
                "UPDATE `items` SET `renamed` = `value` WHERE `value` IS NOT NONE RETURN NONE",
                "UPDATE `items` UNSET `value` RETURN NONE",
            ]
        );
    }

    #[test]
    fn unsafe_changes_emit_guards_instead_of_panicking() {
        let previous = schema("items", "value");
        let mut typed = previous.clone();
        typed.tables[0].columns[1].storage_ty = db::Type::Json;

        let typed_statements = statements(&previous, &typed, &diff::RenameHints::new());
        assert_eq!(typed_statements.len(), 1);
        assert!(typed_statements[0].starts_with("THROW 'manual migration required:"));
        assert!(typed_statements[0].contains("column type change"));

        let renamed = schema("renamed_items", "value");
        let mut hints = diff::RenameHints::new();
        hints.add_table_hint(previous.tables[0].id, renamed.tables[0].id);
        let rename_statements = statements(&previous, &renamed, &hints);
        assert_eq!(rename_statements.len(), 1);
        assert!(rename_statements[0].contains("table rename"));
    }

    #[test]
    fn primary_key_alias_rename_is_a_physical_noop() {
        let previous = schema("items", "value");
        let mut next = previous.clone();
        next.tables[0].columns[0].name = "renamed_id".to_string();
        let mut hints = diff::RenameHints::new();
        hints.add_column_hint(
            previous.tables[0].columns[0].id,
            next.tables[0].columns[0].id,
        );

        assert_eq!(statements(&previous, &next, &hints), vec!["RETURN NONE"]);
    }
}
