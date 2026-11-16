//! SQLite implementation of VCF filters, ordering and source-ID/ordinal mapping.
use super::super::{
    model::{RecordSummary, VcfEntry},
    query::{Query, Sort, SortField},
    reader::invalid,
};
use super::{Dataset, decode_kind, kind_code, sql};
use rusqlite::{OptionalExtension, params, params_from_iter, types::Value};
use std::{io, sync::Arc};

/// Build bound predicates; user text is never interpolated into SQL syntax.
pub(super) fn predicate(query: &Query, contig: Option<&str>) -> (String, Vec<Value>) {
    let mut terms = vec!["1".to_owned()];
    let mut values = Vec::new();
    for value in [contig, query.contig.as_deref()].into_iter().flatten() {
        terms.push("contig=?".into());
        values.push(Value::Text(value.to_owned()));
    }
    if query.pass_only {
        terms.push("filter='PASS'".into());
    }
    if let Some(value) = query.min_qual {
        terms.push("qual>=?".into());
        values.push(Value::Real(value));
    }
    if let Some(value) = query.max_qual {
        terms.push("qual<=?".into());
        values.push(Value::Real(value));
    }
    if let Some(value) = query.kind {
        terms.push("kind=?".into());
        values.push(Value::Integer(kind_code(value).into()));
    }
    if let Some(value) = &query.id {
        terms.push("instr(query_id,?)>0".into());
        values.push(Value::Text(value.clone()));
    }
    if let Some(value) = query.start {
        terms.push("pos>=?".into());
        values.push(Value::Integer(value as i64));
    }
    if let Some(value) = query.end {
        terms.push("pos<=?".into());
        values.push(Value::Integer(value as i64));
    }
    (terms.join(" AND "), values)
}

/// SQL ordering contains only enum-selected syntax, with a stable source-ID tie-breaker.
fn order(sort: Sort) -> String {
    let direction = if sort.descending { "DESC" } else { "ASC" };
    match sort.field {
        SortField::Source => format!("id {direction}"),
        SortField::Position => {
            format!("contig COLLATE BINARY {direction},pos {direction},id ASC")
        }
        SortField::Qual => format!("qual IS NULL ASC,qual {direction},id ASC"),
        SortField::Type => format!("kind {direction},id ASC"),
    }
}

impl Dataset {
    /// Count matching indexed records without loading their summaries into memory.
    pub(crate) fn query_total(&self, contig: Option<&str>, query: &Query) -> io::Result<u64> {
        self.check_source()?;
        query.validate().map_err(invalid)?;
        if query.is_unfiltered() {
            return self.total(contig);
        }
        let (predicate, values) = predicate(query, contig);
        self.db
            .query_row(
                &format!("SELECT COUNT(*) FROM records WHERE {predicate}"),
                params_from_iter(values),
                |row| row.get(0),
            )
            .map_err(sql)
    }

    /// Fetch a bounded SQL window; IDs identify source records and ordinals identify this query.
    /// Sorting uses SQLite's file-backed temporary storage rather than collecting the source.
    pub(crate) fn query_rows(
        &self,
        contig: Option<&str>,
        query: &Query,
        start: u64,
        count: usize,
    ) -> io::Result<Vec<RecordSummary>> {
        self.check_source()?;
        query.validate().map_err(invalid)?;
        if query == &Query::default() {
            return self.rows(contig, start, count);
        }
        let (predicate, mut values) = predicate(query, contig);
        let offset =
            i64::try_from(start).map_err(|_| invalid("Viewport offset exceeds supported range"))?;
        values.extend([
            Value::Integer(count.min(512) as i64),
            Value::Integer(offset),
        ]);
        let statement = format!(
            "SELECT id,contig,pos,ids,reference,alternates,qual,filter,kind
             FROM records WHERE {predicate}
             ORDER BY {} LIMIT ? OFFSET ?",
            order(query.sort)
        );
        let mut statement = self.db.prepare(&statement).map_err(sql)?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok(RecordSummary {
                    id: row.get(0)?,
                    ordinal: 0,
                    chrom: row.get(1)?,
                    pos: row.get(2)?,
                    ids: row.get(3)?,
                    reference: row.get(4)?,
                    alternates: row.get(5)?,
                    qual: row.get(6)?,
                    filter: row.get(7)?,
                    kind: decode_kind(row.get(8)?),
                })
            })
            .map_err(sql)?;
        rows.enumerate()
            .map(|(index, row)| {
                row.map(|mut row| {
                    row.ordinal = start + index as u64;
                    row
                })
                .map_err(sql)
            })
            .collect()
    }

    /// Materialize only the selected record in this query's ordering and restore the scan cursor.
    pub(crate) fn query_entry(
        &mut self,
        contig: Option<&str>,
        query: &Query,
        ordinal: u64,
    ) -> io::Result<Option<Arc<VcfEntry>>> {
        self.check_source()?;
        query.validate().map_err(invalid)?;
        if query == &Query::default() {
            return self.entry(contig, ordinal);
        }
        let (predicate, mut values) = predicate(query, contig);
        values
            .push(Value::Integer(i64::try_from(ordinal).map_err(|_| {
                invalid("Selected ordinal exceeds supported range")
            })?));
        let statement = format!(
            "SELECT id,offset,bytes FROM records WHERE {predicate}
             ORDER BY {} LIMIT 1 OFFSET ?",
            order(query.sort),
        );
        let row = self
            .db
            .query_row(&statement, params_from_iter(values), |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
            .map_err(sql)?;
        self.entry_range(row)
    }

    /// Map a stable source ID to its ordinal in the current filtered ordering.
    /// Unfiltered source order uses the existing index without a window calculation.
    pub(crate) fn query_ordinal(
        &self,
        contig: Option<&str>,
        query: &Query,
        id: u64,
    ) -> io::Result<Option<u64>> {
        self.check_source()?;
        query.validate().map_err(invalid)?;
        if query == &Query::default() {
            return if let Some(contig) = contig {
                self.db
                    .query_row(
                        "SELECT ordinal FROM records WHERE id=?1 AND contig=?2",
                        params![id, contig],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(sql)
            } else {
                self.db
                    .query_row("SELECT id FROM records WHERE id=?1", [id], |row| row.get(0))
                    .optional()
                    .map_err(sql)
            };
        }
        let (predicate, values) = predicate(query, contig);
        let id = i64::try_from(id).map_err(|_| invalid("Source ID exceeds supported range"))?;
        let mut source_values = values.clone();
        source_values.push(Value::Integer(id));
        let source: Option<(String, i64, Option<f64>, u8)> = self
            .db
            .query_row(
                &format!("SELECT contig,pos,qual,kind FROM records WHERE {predicate} AND id=?"),
                params_from_iter(source_values),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(sql)?;
        let Some((chrom, position, qual, kind)) = source else {
            return Ok(None);
        };
        // Count preceding keys instead of sorting the full result again for every scan update.
        let compare = if query.sort.descending { ">" } else { "<" };
        let mut bound = values;
        let preceding = match query.sort.field {
            SortField::Source => {
                bound.push(Value::Integer(id));
                format!("id{compare}?")
            }
            SortField::Position => {
                bound.extend([
                    Value::Text(chrom.clone()),
                    Value::Text(chrom),
                    Value::Integer(position),
                    Value::Integer(position),
                    Value::Integer(id),
                ]);
                format!("contig{compare}? OR (contig=? AND (pos{compare}? OR (pos=? AND id<?)))")
            }
            SortField::Qual => {
                if let Some(qual) = qual {
                    bound.extend([Value::Real(qual), Value::Real(qual), Value::Integer(id)]);
                    format!("qual IS NOT NULL AND (qual{compare}? OR (qual=? AND id<?))")
                } else {
                    bound.push(Value::Integer(id));
                    "qual IS NOT NULL OR (qual IS NULL AND id<?)".into()
                }
            }
            SortField::Type => {
                bound.extend([
                    Value::Integer(kind.into()),
                    Value::Integer(kind.into()),
                    Value::Integer(id),
                ]);
                format!("kind{compare}? OR (kind=? AND id<?)")
            }
        };
        self.db
            .query_row(
                &format!("SELECT COUNT(*) FROM records WHERE {predicate} AND ({preceding})"),
                params_from_iter(bound),
                |row| row.get(0),
            )
            .map(Some)
            .map_err(sql)
    }

    /// Locate a coordinate within matching records and return its current sorted ordinal.
    /// Non-exact targets wait for EOF, preserving correct navigation for unsorted sources.
    pub(crate) fn query_locate(
        &self,
        contig: &str,
        query: &Query,
        position: u64,
    ) -> io::Result<Option<u64>> {
        self.check_source()?;
        query.validate().map_err(invalid)?;
        if query == &Query::default() {
            return self.locate(contig, position);
        }
        let position = i64::try_from(position)
            .map_err(|_| invalid("Position exceeds supported integer range"))?;
        let (predicate, values) = predicate(query, Some(contig));
        let find = |condition: &str, ordering: &str| -> io::Result<Option<u64>> {
            let mut bound = values.clone();
            if condition.contains('?') {
                bound.push(Value::Integer(position));
            }
            let statement = format!(
                "SELECT id FROM records WHERE {predicate} AND {condition}
                 ORDER BY {ordering} LIMIT 1"
            );
            self.db
                .query_row(&statement, params_from_iter(bound), |row| row.get(0))
                .optional()
                .map_err(sql)
        };
        let exact = find("pos=?", "id ASC")?;
        let source = if exact.is_some() {
            exact
        } else if !self.progress.complete {
            None
        } else {
            find("pos>=?", "pos ASC,id ASC")?.or(find("1", "pos DESC,id DESC")?)
        };
        let Some(source) = source else {
            return Ok(None);
        };
        self.query_ordinal(Some(contig), query, source)
    }
}
