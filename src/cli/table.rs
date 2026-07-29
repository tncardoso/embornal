//! Tables for the terminal.
//!
//! The table sets each column to the width of its widest cell, the heading
//! included, in the way that the `tabwriter` package of Go does. A ruler comes
//! below the heading:
//!
//! ```text
//! | Path | Facts | Children |
//! +------+-------+----------+
//! | path |     0 |        0 |
//! ```

use crate::cli::write_error;
use crate::error::Result;
use std::io::Write;

/// Where the text of a column sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Words read from the left.
    #[default]
    Left,
    /// Numbers line up on the right.
    Right,
}

/// A table that grows one row at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    headings: Vec<String>,
    aligns: Vec<Align>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// Builds a table with these columns.
    ///
    /// Each column carries its heading and where its text sits.
    pub fn new(columns: &[(&str, Align)]) -> Self {
        Self {
            headings: columns
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect(),
            aligns: columns.iter().map(|(_, align)| *align).collect(),
            rows: Vec::new(),
        }
    }

    /// Adds one row.
    ///
    /// A row that is shorter than the heading gets empty cells, and a row that
    /// is longer loses its tail. A table always prints as a rectangle.
    pub fn row<I, S>(&mut self, cells: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut row: Vec<String> = cells.into_iter().map(Into::into).collect();
        row.resize(self.headings.len(), String::new());
        self.rows.push(row);
        self
    }

    /// Returns the number of rows, the heading not counted.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Returns `true` if the table holds no row.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Returns the width of each column.
    fn widths(&self) -> Vec<usize> {
        let mut widths: Vec<usize> = self.headings.iter().map(|h| width(h)).collect();
        for row in &self.rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(width(cell));
            }
        }
        widths
    }

    /// Writes the table.
    pub fn render(&self, out: &mut impl Write) -> Result<()> {
        let widths = self.widths();

        writeln!(out, "{}", self.line(&self.headings, &widths)).map_err(write_error)?;
        writeln!(out, "{}", ruler(&widths)).map_err(write_error)?;
        for row in &self.rows {
            writeln!(out, "{}", self.line(row, &widths)).map_err(write_error)?;
        }
        Ok(())
    }

    /// Builds one line of cells.
    fn line(&self, cells: &[String], widths: &[usize]) -> String {
        let mut line = String::from("|");
        for (index, cell) in cells.iter().enumerate() {
            let pad = widths[index].saturating_sub(width(cell));
            match self.aligns[index] {
                Align::Left => line.push_str(&format!(" {cell}{} |", " ".repeat(pad))),
                Align::Right => line.push_str(&format!(" {}{cell} |", " ".repeat(pad))),
            }
        }
        line
    }
}

/// Builds the line below the heading.
fn ruler(widths: &[usize]) -> String {
    let mut line = String::from("+");
    for width in widths {
        line.push_str(&"-".repeat(width + 2));
        line.push('+');
    }
    line
}

/// Returns how wide a cell prints.
///
/// The count is in characters, so a letter that carries an accent takes one
/// column, as it does on the screen.
fn width(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(table: &Table) -> String {
        let mut buffer = Vec::new();
        table.render(&mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    #[test]
    fn builds_the_shape_of_the_example() {
        let mut table = Table::new(&[
            ("Path", Align::Left),
            ("Facts", Align::Right),
            ("Children", Align::Right),
        ]);
        table.row(["path", "0", "0"]);

        assert_eq!(
            render(&table),
            "| Path | Facts | Children |\n\
             +------+-------+----------+\n\
             | path |     0 |        0 |\n"
        );
    }

    #[test]
    fn a_column_grows_to_its_widest_cell() {
        let mut table = Table::new(&[("Path", Align::Left), ("Facts", Align::Right)]);
        table.row(["a", "1"]);
        table.row(["a-much-longer-name", "1000000"]);

        let output = render(&table);
        let lines: Vec<&str> = output.lines().collect();
        // Every line is exactly as wide as the ruler.
        for line in &lines {
            assert_eq!(line.chars().count(), lines[1].chars().count(), "{line}");
        }
        assert!(lines[0].starts_with("| Path               |"));
    }

    #[test]
    fn numbers_line_up_on_the_right() {
        let mut table = Table::new(&[("N", Align::Right)]);
        table.row(["1"]);
        table.row(["100"]);

        let output = render(&table);
        assert!(output.contains("|   1 |"));
        assert!(output.contains("| 100 |"));
    }

    #[test]
    fn words_read_from_the_left() {
        let mut table = Table::new(&[("Name", Align::Left)]);
        table.row(["a"]);
        table.row(["abcd"]);

        let output = render(&table);
        assert!(output.contains("| a    |"));
        assert!(output.contains("| abcd |"));
    }

    #[test]
    fn a_table_with_no_row_shows_its_heading() {
        let table = Table::new(&[("Path", Align::Left), ("Facts", Align::Right)]);
        assert!(table.is_empty());
        assert_eq!(render(&table), "| Path | Facts |\n+------+-------+\n");
    }

    #[test]
    fn a_short_row_gets_empty_cells() {
        let mut table = Table::new(&[("A", Align::Left), ("B", Align::Left)]);
        table.row(["only"]);
        assert_eq!(render(&table), "| A    | B |\n+------+---+\n| only |   |\n");
    }

    #[test]
    fn a_long_row_loses_its_tail() {
        let mut table = Table::new(&[("A", Align::Left)]);
        table.row(["one", "two"]);
        assert_eq!(render(&table), "| A   |\n+-----+\n| one |\n");
    }

    #[test]
    fn a_letter_with_an_accent_takes_one_column() {
        let mut table = Table::new(&[("Path", Align::Left)]);
        table.row(["memória"]);
        table.row(["abcdefg"]);

        let output = render(&table);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(
            lines[2].chars().count(),
            lines[3].chars().count(),
            "two names of seven letters must print as wide"
        );
    }

    #[test]
    fn counts_its_rows() {
        let mut table = Table::new(&[("A", Align::Left)]);
        assert_eq!(table.len(), 0);
        table.row(["one"]).row(["two"]);
        assert_eq!(table.len(), 2);
    }
}
