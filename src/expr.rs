//! Translation of Toasty filter/condition [`stmt::Expr`] trees into SurrealQL
//! predicate strings.
//!
//! Mirrors the DynamoDB driver's `ddb_expression`: it walks the expression,
//! resolves column references to SurrealQL field references, and appends scalar
//! literals as bound parameters (`$p0`, `$p1`, …) collected into [`Binds`].
//!
//! SurrealDB stores a row's primary key **only** in its record id, not as a
//! regular field, so a reference to a primary-key column is rendered through
//! `record::id(id)` (or `record::id(id)[k]` for a composite key element) rather
//! than as a bare field name.
//!
//! Unsupported expression shapes return
//! [`toasty_core::Error::unsupported_feature`] rather than panicking, keeping
//! the driver robust against `stmt::Expr` growing new variants.

use surrealdb::types::Value as SurValue;
use toasty_core::schema::db::{self, Column, Table};
use toasty_core::stmt::{self, BinaryOp, ExprContext};

use crate::value::{to_surreal, to_surreal_for_storage};

/// Accumulates bound parameters produced while rendering an expression.
///
/// Parameter names are stable positional identifiers (`p0`, `p1`, …) so the
/// generated SurrealQL and the bind list stay in lockstep.
#[derive(Default)]
pub(crate) struct Binds {
    values: Vec<(String, SurValue)>,
}

impl Binds {
    /// Registers a value and returns its `$pN` placeholder.
    pub(crate) fn push(&mut self, value: SurValue) -> String {
        let name = format!("p{}", self.values.len());
        let placeholder = format!("${name}");
        self.values.push((name, value));
        placeholder
    }

    /// Registers a value under an explicit parameter name (without the `$`).
    ///
    /// Used for the record-id parameter, which the SQL text references by a
    /// fixed name (`$rid`) rather than a positional placeholder.
    pub(crate) fn push_named(&mut self, name: impl Into<String>, value: SurValue) {
        self.values.push((name.into(), value));
    }

    /// Consumes the accumulated `(name, value)` bindings.
    pub(crate) fn into_vec(self) -> Vec<(String, SurValue)> {
        self.values
    }
}

/// Returns `true` when `column` is part of the table's primary key.
///
/// The schema builder leaves `Column::primary_key` as `false`; primary-key
/// membership is recorded only in `Table::primary_key.columns`, so that is the
/// authoritative source.
pub(crate) fn is_primary_key(table: &Table, column: &Column) -> bool {
    table.primary_key.columns.contains(&column.id)
}

/// Renders the SurrealQL reference for a resolved column.
///
/// A primary-key column lives in the record id, so it is projected through
/// `record::id(id)`; composite keys index into that array by the column's
/// position in the primary key.
pub(crate) fn column_ref(table: &Table, column: &Column) -> String {
    if is_primary_key(table, column) {
        let pk = &table.primary_key.columns;
        if pk.len() <= 1 {
            "record::id(id)".to_string()
        } else {
            let pos = pk.iter().position(|id| *id == column.id).unwrap_or(0);
            format!("record::id(id)[{pos}]")
        }
    } else {
        escape_ident(&column.name)
    }
}

/// Renders `expr` as a SurrealQL predicate, collecting literals into `binds`.
pub(crate) fn render(
    cx: &ExprContext<'_, db::Schema>,
    table: &Table,
    binds: &mut Binds,
    expr: &stmt::Expr,
) -> toasty_core::Result<String> {
    render_with_storage(cx, table, binds, expr, None)
}

/// Internal renderer carrying a storage-type hint for literal nodes. The hint
/// comes from the column on the other side of a comparison and is essential
/// for native JSON, whose Toasty wire value is otherwise indistinguishable
/// from an ordinary string.
fn render_with_storage(
    cx: &ExprContext<'_, db::Schema>,
    table: &Table,
    binds: &mut Binds,
    expr: &stmt::Expr,
    storage_ty: Option<&db::Type>,
) -> toasty_core::Result<String> {
    match expr {
        stmt::Expr::BinaryOp(op) => {
            let lhs_storage = expression_storage_ty(cx, &op.lhs);
            let rhs_storage = expression_storage_ty(cx, &op.rhs);
            let lhs = render_with_storage(cx, table, binds, &op.lhs, rhs_storage.as_ref())?;
            let rhs = render_with_storage(cx, table, binds, &op.rhs, lhs_storage.as_ref())?;
            let sym = match op.op {
                BinaryOp::Eq => "=",
                BinaryOp::Ne => "!=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                // Defensive: `BinaryOp` is not `#[non_exhaustive]`, so this is
                // unreachable today but future-proofs against a new operator.
                #[allow(unreachable_patterns)]
                other => {
                    return Err(toasty_core::Error::unsupported_feature(format!(
                        "SurrealDB driver does not support binary operator {other:?}"
                    )));
                }
            };
            Ok(format!("{lhs} {sym} {rhs}"))
        }
        stmt::Expr::And(and) => {
            let parts = render_operands(cx, table, binds, &and.operands)?;
            Ok(format!("({})", parts.join(" AND ")))
        }
        stmt::Expr::Or(or) => {
            let parts = render_operands(cx, table, binds, &or.operands)?;
            Ok(format!("({})", parts.join(" OR ")))
        }
        stmt::Expr::Not(not) => {
            let inner = render_with_storage(cx, table, binds, &not.expr, None)?;
            // SurrealQL rejects `NOT x IS NONE`; use the `!( ... )` prefix form,
            // which parses correctly around any predicate.
            Ok(format!("!({inner})"))
        }
        stmt::Expr::IsNull(is_null) => {
            // Render the operand as a bare field reference. A bool column would
            // otherwise render as `col = true` (predicate position), producing
            // the invalid `col = true IS NONE`; presence checks need the plain
            // field. Document-path leaves resolve to a dotted reference.
            let inner = match is_null.expr.as_ref() {
                stmt::Expr::Reference(reference) => {
                    let column = cx.resolve_expr_reference(reference).as_column_unwrap();
                    column_ref(table, column)
                }
                stmt::Expr::Func(stmt::ExprFunc::JsonExtract(func)) => {
                    document_path(cx, table, func)
                }
                other => render_with_storage(cx, table, binds, other, None)?,
            };
            Ok(format!("{inner} IS NONE"))
        }
        stmt::Expr::Between(between) => {
            let target_storage = expression_storage_ty(cx, &between.expr);
            let target = render_with_storage(cx, table, binds, &between.expr, None)?;
            let low = render_with_storage(cx, table, binds, &between.low, target_storage.as_ref())?;
            let high =
                render_with_storage(cx, table, binds, &between.high, target_storage.as_ref())?;
            // Render as two comparisons; SurrealQL has no chained relational
            // operator with defined associativity.
            Ok(format!("({target} >= {low} AND {target} <= {high})"))
        }
        stmt::Expr::InList(in_list) => {
            let target_storage = expression_storage_ty(cx, &in_list.expr);
            let target = render_with_storage(cx, table, binds, &in_list.expr, None)?;
            let list =
                render_with_storage(cx, table, binds, &in_list.list, target_storage.as_ref())?;
            Ok(format!("{target} IN {list}"))
        }
        stmt::Expr::AnyOp(any) => {
            // `value <op> ANY(col)`. For equality this is list membership:
            // `col CONTAINS value`. Other operators have no direct SurrealQL
            // list form.
            if any.op != BinaryOp::Eq {
                return Err(toasty_core::Error::unsupported_feature(format!(
                    "SurrealDB driver only supports `= ANY` list membership, got {:?}",
                    any.op
                )));
            }
            let rhs_storage = expression_storage_ty(cx, &any.rhs);
            let value = render_with_storage(cx, table, binds, &any.lhs, rhs_storage.as_ref())?;
            let list = render_with_storage(cx, table, binds, &any.rhs, None)?;
            Ok(format!("{list} CONTAINS {value}"))
        }
        stmt::Expr::IsSuperset(sup) => {
            let lhs_storage = expression_storage_ty(cx, &sup.lhs);
            let rhs_storage = expression_storage_ty(cx, &sup.rhs);
            let lhs = render_with_storage(cx, table, binds, &sup.lhs, rhs_storage.as_ref())?;
            let rhs = render_with_storage(cx, table, binds, &sup.rhs, lhs_storage.as_ref())?;
            Ok(format!("{lhs} CONTAINSALL {rhs}"))
        }
        stmt::Expr::Intersects(int) => {
            let lhs_storage = expression_storage_ty(cx, &int.lhs);
            let rhs_storage = expression_storage_ty(cx, &int.rhs);
            let lhs = render_with_storage(cx, table, binds, &int.lhs, rhs_storage.as_ref())?;
            let rhs = render_with_storage(cx, table, binds, &int.rhs, lhs_storage.as_ref())?;
            Ok(format!("{lhs} CONTAINSANY {rhs}"))
        }
        stmt::Expr::Length(len) => {
            let inner = render_with_storage(cx, table, binds, &len.expr, None)?;
            Ok(format!("array::len({inner})"))
        }
        stmt::Expr::StartsWith(sw) => {
            let target_storage = expression_storage_ty(cx, &sw.expr);
            let target = render_with_storage(cx, table, binds, &sw.expr, None)?;
            let prefix =
                render_with_storage(cx, table, binds, &sw.prefix, target_storage.as_ref())?;
            // Guard against NULL/absent values: `string::starts_with` errors on
            // a non-string argument, and an absent optional field never matches
            // a prefix.
            Ok(format!(
                "({target} IS NOT NONE AND string::starts_with({target}, {prefix}))"
            ))
        }
        stmt::Expr::Func(stmt::ExprFunc::JsonExtract(func)) => {
            // A projection into a `#[document]` column arrives as a base column
            // plus a key path. SurrealQL addresses nested object fields with
            // dotted identifiers.
            Ok(document_path(cx, table, func))
        }
        stmt::Expr::Reference(reference) => {
            let column = cx.resolve_expr_reference(reference).as_column_unwrap();
            let reference = column_ref(table, column);
            // A bare boolean column used as a predicate means "= true".
            if column.ty == stmt::Type::Bool {
                Ok(format!("{reference} = true"))
            } else {
                Ok(reference)
            }
        }
        stmt::Expr::Value(value) => value_to_binds(binds, value, storage_ty),
        stmt::Expr::List(list) => {
            let mut parts = Vec::with_capacity(list.items.len());
            for item in &list.items {
                parts.push(render_with_storage(cx, table, binds, item, storage_ty)?);
            }
            Ok(format!("[{}]", parts.join(", ")))
        }
        other => Err(toasty_core::Error::unsupported_feature(format!(
            "SurrealDB driver cannot translate filter expression: {other:?}"
        ))),
    }
}

/// Resolves the database storage type carried by a column-shaped expression.
/// Returning an owned type keeps the renderer's lifetimes simple and is cheap
/// compared with issuing the database query.
fn expression_storage_ty(cx: &ExprContext<'_, db::Schema>, expr: &stmt::Expr) -> Option<db::Type> {
    match expr {
        stmt::Expr::Reference(reference) => Some(
            cx.resolve_expr_reference(reference)
                .as_column_unwrap()
                .storage_ty
                .clone(),
        ),
        stmt::Expr::Func(stmt::ExprFunc::JsonExtract(func)) => {
            expression_storage_ty(cx, &func.base)
        }
        _ => None,
    }
}

/// Renders a lowered document path (`FuncJsonExtract`) as a dotted SurrealQL
/// field reference (`col.a.b`), escaping each segment.
fn document_path(
    cx: &ExprContext<'_, db::Schema>,
    table: &Table,
    func: &stmt::FuncJsonExtract,
) -> String {
    let base = match func.base.as_ref() {
        stmt::Expr::Reference(reference) => {
            let column = cx.resolve_expr_reference(reference).as_column_unwrap();
            column_ref(table, column)
        }
        // Unusual, but fall back to a generic render of the base.
        other => {
            let mut binds = Binds::default();
            render(cx, table, &mut binds, other).unwrap_or_else(|_| "id".to_string())
        }
    };
    let mut path = base;
    for segment in &func.path {
        path.push('.');
        path.push_str(&escape_ident(segment));
    }
    path
}

/// Renders each operand of an AND/OR node.
fn render_operands(
    cx: &ExprContext<'_, db::Schema>,
    table: &Table,
    binds: &mut Binds,
    operands: &[stmt::Expr],
) -> toasty_core::Result<Vec<String>> {
    operands
        .iter()
        .map(|op| render(cx, table, binds, op))
        .collect()
}

/// Binds a literal value. A [`stmt::Value::List`] renders as a SurrealQL array
/// literal so `IN` receives a real list operand rather than one opaque param.
fn value_to_binds(
    binds: &mut Binds,
    value: &stmt::Value,
    storage_ty: Option<&db::Type>,
) -> toasty_core::Result<String> {
    match value {
        stmt::Value::List(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                let value = match storage_ty {
                    Some(storage_ty) => to_surreal_for_storage(item, storage_ty)?,
                    None => to_surreal(item)?,
                };
                parts.push(binds.push(value));
            }
            Ok(format!("[{}]", parts.join(", ")))
        }
        other => {
            let value = match storage_ty {
                Some(storage_ty) => to_surreal_for_storage(other, storage_ty)?,
                None => to_surreal(other)?,
            };
            Ok(binds.push(value))
        }
    }
}

/// Wraps an identifier in backticks, escaping any embedded backticks, so column
/// names that collide with SurrealQL keywords remain valid.
pub(crate) fn escape_ident(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}
