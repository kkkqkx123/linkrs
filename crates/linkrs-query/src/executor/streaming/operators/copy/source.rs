use std::fs::File;
use std::io::BufReader;

use linkrs_core::error::QueryError;

use super::from::ColumnMapping;

pub(crate) struct CsvSource {
    headers: Vec<String>,
    reader: csv::StringRecordsIntoIter<std::io::BufReader<File>>,
    finished: bool,
}

impl CsvSource {
    pub(crate) fn open(file_path: &str, header: bool, delimiter: u8) -> Result<Self, QueryError> {
        let file = File::open(file_path).map_err(|e| {
            QueryError::execution(format!("COPY FROM failed to open '{file_path}': {e}"))
        })?;
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(header)
            .delimiter(delimiter)
            .trim(csv::Trim::All)
            .flexible(true)
            .from_reader(BufReader::new(file));
        let headers: Vec<String> = if header {
            reader
                .headers()
                .map_err(|e| QueryError::execution(format!("COPY CSV header error: {e}")))?
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            Vec::new()
        };
        Ok(Self {
            headers,
            reader: reader.into_records(),
            finished: false,
        })
    }

    /// Header names when the import declared `HEADER`; empty otherwise.
    pub(crate) fn headers(&self) -> &[String] {
        &self.headers
    }

    pub(crate) fn is_eof(&self) -> bool {
        self.finished
    }

    /// Pull up to `batch_size` records as plain string rows.
    ///
    /// Records whose field count differs from the expected width are counted
    /// as malformed and skipped (reported once at the end of the import).
    pub(crate) fn next_batch(
        &mut self,
        batch_size: usize,
        mapping: &ColumnMapping,
        skipped: &mut u64,
    ) -> Result<Vec<Vec<String>>, QueryError> {
        let mut out: Vec<Vec<String>> = Vec::with_capacity(batch_size);
        while out.len() < batch_size {
            let Some(record) = self.next_record()? else {
                break;
            };
            if record.len() != mapping.expected_width {
                *skipped += 1;
                continue;
            }
            out.push(record.iter().map(|s| s.to_string()).collect());
        }
        Ok(out)
    }

    fn next_record(&mut self) -> Result<Option<csv::StringRecord>, QueryError> {
        if self.finished {
            return Ok(None);
        }
        match self.reader.next() {
            Some(Ok(rec)) => Ok(Some(rec)),
            Some(Err(e)) => Err(QueryError::execution(format!("COPY CSV read error: {e}"))),
            None => {
                self.finished = true;
                Ok(None)
            }
        }
    }
}

/// Multi-file CSV source supporting row concatenation (default) and
/// column-wise merge (`BY COLUMN`).
pub(crate) enum MergedCsvSource {
    /// Files concatenated row-wise: same schema, rows appended in order.
    Rows {
        headers: Vec<String>,
        sources: Vec<CsvSource>,
        current: usize,
    },
    /// Files merged column-wise: headers concatenated, rows zipped by
    /// position. All files must carry the same data row count.
    Columns {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        pos: usize,
    },
}

impl MergedCsvSource {
    pub(crate) fn open(
        file_paths: &[String],
        by_column: bool,
        header: bool,
        delimiter: u8,
    ) -> Result<Self, QueryError> {
        if file_paths.is_empty() {
            return Err(QueryError::execution(
                "COPY FROM requires at least one file".to_string(),
            ));
        }
        if !by_column {
            let mut sources = Vec::with_capacity(file_paths.len());
            for path in file_paths {
                sources.push(CsvSource::open(path, header, delimiter)?);
            }
            let headers = sources
                .first()
                .map(|s| s.headers().to_vec())
                .unwrap_or_default();
            if header {
                for (idx, source) in sources.iter().enumerate().skip(1) {
                    if source.headers() != headers.as_slice() {
                        return Err(QueryError::execution(format!(
                            "COPY FROM: header mismatch between '{}' and '{}'",
                            file_paths[0], file_paths[idx]
                        )));
                    }
                }
            }
            return Ok(Self::Rows {
                headers,
                sources,
                current: 0,
            });
        }

        // Column-wise merge: load every file, then zip rows by position.
        let mut per_file_headers: Vec<Vec<String>> = Vec::with_capacity(file_paths.len());
        let mut per_file_rows: Vec<Vec<Vec<String>>> = Vec::with_capacity(file_paths.len());
        for path in file_paths {
            let mut source = CsvSource::open(path, header, delimiter)?;
            let file_headers = source.headers().to_vec();
            let file_width = file_headers.len();
            let mut rows: Vec<Vec<String>> = Vec::new();
            while let Some(record) = source.next_record()? {
                rows.push(record.iter().map(|s| s.to_string()).collect());
            }
            if header {
                for (idx, row) in rows.iter().enumerate() {
                    if row.len() != file_width {
                        return Err(QueryError::execution(format!(
                            "COPY FROM BY COLUMN: file '{path}' row {} has {} fields, expected {file_width}",
                            idx + 1,
                            row.len()
                        )));
                    }
                }
            }
            per_file_headers.push(file_headers);
            per_file_rows.push(rows);
        }
        let row_count = per_file_rows.first().map(|r| r.len()).unwrap_or(0);
        for (idx, rows) in per_file_rows.iter().enumerate().skip(1) {
            if rows.len() != row_count {
                return Err(QueryError::execution(format!(
                    "COPY FROM BY COLUMN: file '{}' has {} rows, expected {row_count} (from '{}')",
                    file_paths[idx],
                    rows.len(),
                    file_paths[0]
                )));
            }
        }
        let headers: Vec<String> = per_file_headers.into_iter().flatten().collect();
        let mut rows: Vec<Vec<String>> = Vec::with_capacity(row_count);
        for row_idx in 0..row_count {
            let mut merged: Vec<String> = Vec::new();
            for file_rows in &per_file_rows {
                merged.extend(file_rows[row_idx].iter().cloned());
            }
            rows.push(merged);
        }
        Ok(Self::Columns {
            headers,
            rows,
            pos: 0,
        })
    }

    pub(crate) fn headers(&self) -> &[String] {
        match self {
            Self::Rows { headers, .. } => headers,
            Self::Columns { headers, .. } => headers,
        }
    }

    pub(crate) fn is_eof(&self) -> bool {
        match self {
            Self::Rows {
                sources, current, ..
            } => *current >= sources.len(),
            Self::Columns { rows, pos, .. } => *pos >= rows.len(),
        }
    }

    pub(crate) fn next_batch(
        &mut self,
        batch_size: usize,
        mapping: &ColumnMapping,
        skipped: &mut u64,
    ) -> Result<Vec<Vec<String>>, QueryError> {
        match self {
            Self::Rows {
                sources, current, ..
            } => {
                let mut out: Vec<Vec<String>> = Vec::with_capacity(batch_size);
                while out.len() < batch_size && *current < sources.len() {
                    let batch =
                        sources[*current].next_batch(batch_size - out.len(), mapping, skipped)?;
                    if batch.is_empty() {
                        if sources[*current].is_eof() {
                            *current += 1;
                            continue;
                        }
                        break;
                    }
                    out.extend(batch);
                }
                Ok(out)
            }
            Self::Columns { rows, pos, .. } => {
                let mut out: Vec<Vec<String>> = Vec::with_capacity(batch_size);
                while out.len() < batch_size && *pos < rows.len() {
                    let row = &rows[*pos];
                    *pos += 1;
                    if row.len() != mapping.expected_width {
                        *skipped += 1;
                        continue;
                    }
                    out.push(row.clone());
                }
                Ok(out)
            }
        }
    }
}
