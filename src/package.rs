use rusqlite::{Connection, Transaction};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;
use zip::{write::FileOptions, ZipWriter};

use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

use crate::apkg_col::APKG_COL;
use crate::apkg_schema::APKG_SCHEMA;
use crate::deck::Deck;
use crate::error::{database_error, json_error, zip_error};
use crate::Error;
use std::str::FromStr;

/// `Package` to pack `Deck`s and `media_files` and write them to a `.apkg` file
///
/// Example:
/// ```rust
/// use genanki_rs::{Package, Deck, Note, Model, Field, Template};
///
/// let model = Model::new(
///     1607392319,
///     "Simple Model",
///     vec![
///         Field::new("Question"),
///         Field::new("Answer"),
///         Field::new("MyMedia"),
///     ],
///     vec![Template::new("Card 1")
///         .qfmt("{{Question}}{{Question}}<br>{{MyMedia}}")
///         .afmt(r#"{{FrontSide}}<hr id="answer">{{Answer}}"#)],
/// );
///
/// let mut deck = Deck::new(1234, "Example Deck", "Example Deck with media");
/// deck.add_note(Note::new(&model, vec!["What is the capital of France.unwrap()", "Paris", "[sound:sound.mp3]"]).unwrap());
/// deck.add_note(Note::new(&model, vec!["What is the capital of France.unwrap()", "Paris", r#"<img src="image.jpg">"#]).unwrap());
///
/// let mut package = Package::new(vec![deck], vec![/*"sound.mp3", "images/image.jpg"*/]).unwrap();
/// package.write_to_file("output.apkg").unwrap();
/// ```
pub struct Package<'a> {
    decks: Vec<Deck<'a>>,
    media_files: Vec<PathBuf>,
}

impl<'a> Package<'a> {
    /// Create a new package with `decks` and `media_files`
    ///
    /// Returns `Err` if `media_files` are invalid
    pub fn new(decks: Vec<Deck<'a>>, media_files: Vec<&str>) -> Result<Self, Error> {
        let media_files = media_files
            .iter()
            .map(|&s| PathBuf::from_str(s))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { decks, media_files })
    }

    /// Writes the package to a writer
    ///
    /// Returns `Err` if an IO error occurrs
    pub fn write_to<W>(&mut self, out: W) -> Result<(), Error>
    where
        W: Write + Seek,
    {
        self.write_to_maybe_timestamp(out, None)
    }

    /// Writes the package to a file
    ///
    /// Returns `Err` if the `file` cannot be created
    pub fn write_to_file(&mut self, file: &str) -> Result<(), Error> {
        self.write_to_file_maybe_timestamp(file, None)
    }

    /// Writes the package to a writer using a timestamp
    ///
    /// Returns `Err` if an IO error occurrs
    pub fn write_to_timestamp<W>(&mut self, out: W, timestamp: f64) -> Result<(), Error>
    where
        W: Write + Seek,
    {
        self.write_to_maybe_timestamp(out, Some(timestamp))
    }

    /// Writes the package to a file using a timestamp
    ///
    /// Returns `Err` if the `file` cannot be created
    pub fn write_to_file_timestamp(&mut self, file: &str, timestamp: f64) -> Result<(), Error> {
        self.write_to_file_maybe_timestamp(file, Some(timestamp))
    }

    fn write_to_file_maybe_timestamp(
        &mut self,
        file: &str,
        timestamp: Option<f64>,
    ) -> Result<(), Error> {
        let file = File::create(&file)?;
        self.write_to_maybe_timestamp(file, timestamp)?;
        Ok(())
    }

    fn write_to_maybe_timestamp<W>(&mut self, out: W, timestamp: Option<f64>) -> Result<(), Error>
    where
        W: Write + Seek,
    {
        let db_file = NamedTempFile::new()?.into_temp_path();

        let mut conn = Connection::open(&db_file).map_err(database_error)?;
        let transaction = conn.transaction().map_err(database_error)?;

        let timestamp = timestamp.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|i| i.as_secs_f64())
                .unwrap_or(0.0)
        });

        self.write_to_db(&transaction, timestamp)?;
        transaction.commit().map_err(database_error)?;
        conn.close().expect("Should always close");

        let mut outzip = ZipWriter::new(out);
        outzip
            .start_file("collection.anki2", FileOptions::default())
            .map_err(zip_error)?;
        outzip.write_all(&read_file_bytes(db_file)?)?;

        let media_file_idx_to_path = self
            .media_files
            .iter()
            .enumerate()
            .collect::<HashMap<usize, &PathBuf>>();
        let media_map = media_file_idx_to_path
            .clone()
            .into_iter()
            .map(|(id, path)| {
                (
                    id.to_string(),
                    path.file_name()
                        .expect("Should always have a filename")
                        .to_str()
                        .expect("should always have string"),
                )
            })
            .collect::<HashMap<String, &str>>();
        let media_json = serde_json::to_string(&media_map).map_err(json_error)?;
        outzip
            .start_file("media", FileOptions::default())
            .map_err(zip_error)?;
        outzip.write_all(media_json.as_bytes())?;

        for (idx, &path) in &media_file_idx_to_path {
            outzip
                .start_file(idx.to_string(), FileOptions::default())
                .map_err(zip_error)?;
            outzip.write_all(&read_file_bytes(path)?)?;
        }
        outzip.finish().map_err(zip_error)?;
        Ok(())
    }

    fn write_to_db(&mut self, transaction: &Transaction, timestamp: f64) -> Result<(), Error> {
        let mut id_gen = ((timestamp * 1000.0) as usize)..;
        transaction
            .execute_batch(APKG_SCHEMA)
            .map_err(database_error)?;
        transaction
            .execute_batch(APKG_COL)
            .map_err(database_error)?;
        for deck in &mut self.decks {
            deck.write_to_db(&transaction, timestamp, &mut id_gen)?;
        }
        Ok(())
    }
}

#[inline]
fn read_file_bytes<P: AsRef<Path>>(path: P) -> Result<Vec<u8>, Error> {
    Ok(std::fs::read(path)?)
}
