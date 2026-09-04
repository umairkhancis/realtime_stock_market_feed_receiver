//! The filesystem adapters behind [`FeedStore`] and [`SymbolSource`].
//!
//! Everything path-shaped in the program is concentrated here: this is the only
//! file that names `File`, `PathBuf` or `create_dir_all`. The use cases hold a
//! `&impl FeedStore` and never learn that `data/received.csv` exists.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::application::ports::{FeedStore, StoredFeed, SymbolSource};
use crate::domain::message::ItchMessage;
use crate::domain::symbols::SymbolMap;
use crate::infrastructure::csv::serde::{FeedError, read_feed, read_symbol_table, write_feed};

/// A feed CSV in the transmitter's 14-column format.
#[derive(Debug, Clone)]
pub struct CsvFeedStore {
    path: PathBuf,
}

impl CsvFeedStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        CsvFeedStore { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl FeedStore for CsvFeedStore {
    type Error = FeedError;

    fn location(&self) -> String {
        self.path.display().to_string()
    }

    fn load(&self) -> Result<Vec<ItchMessage>, FeedError> {
        read_feed(BufReader::new(File::open(&self.path)?))
    }

    fn save(&self, messages: &[ItchMessage]) -> Result<StoredFeed, FeedError> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let mut out = BufWriter::new(File::create(&self.path)?);
        let rows = write_feed(&mut out, messages.iter().copied())?;
        out.flush()?;

        Ok(StoredFeed { rows, location: self.path.display().to_string() })
    }
}

/// The locate → ticker map the transmitter writes as `<feed>.symbols.csv`.
#[derive(Debug, Clone)]
pub struct CsvSymbolSource {
    path: PathBuf,
}

impl CsvSymbolSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        CsvSymbolSource { path: path.into() }
    }

    /// `data/feed.csv` -> `data/feed.symbols.csv`, the convention the
    /// transmitter's own store writes to.
    ///
    /// The map sits beside the feed rather than inside it because the two have
    /// different lifetimes: a receiver may keep one map across many captures.
    pub fn beside(feed: &Path) -> Self {
        let stem = feed
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        CsvSymbolSource::new(feed.with_file_name(format!("{stem}.symbols.csv")))
    }
}

impl SymbolSource for CsvSymbolSource {
    type Error = FeedError;

    fn location(&self) -> String {
        self.path.display().to_string()
    }

    fn load(&self) -> Result<SymbolMap, FeedError> {
        let map = read_symbol_table(BufReader::new(File::open(&self.path)?))?;
        Ok(SymbolMap::new(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_symbol_map_sits_beside_the_feed() {
        assert_eq!(
            CsvSymbolSource::beside(Path::new("data/feed.csv")).location(),
            "data/feed.symbols.csv"
        );
        assert_eq!(
            CsvSymbolSource::beside(Path::new("run1.csv")).location(),
            "run1.symbols.csv"
        );
    }

    /// A store reports where it reads and writes without touching the disk to
    /// find out — the location is what every one of this crate's report lines
    /// prints, and it must not depend on the file existing.
    #[test]
    fn a_store_names_its_location_without_opening_it() {
        let store = CsvFeedStore::new("data/does-not-exist.csv");
        assert_eq!(store.location(), "data/does-not-exist.csv");
        assert_eq!(store.path(), Path::new("data/does-not-exist.csv"));
        assert!(store.load().is_err());
    }

    #[test]
    fn a_feed_survives_a_round_trip_through_a_real_file() {
        let msgs = crate::domain::fixtures::synthetic(500);
        let dir = std::env::temp_dir().join(format!("rx-store-{}", std::process::id()));
        let store = CsvFeedStore::new(dir.join("received.csv"));

        let stored = store.save(&msgs).unwrap();
        assert_eq!(stored.rows, 500);
        assert_eq!(stored.location, store.location());
        assert_eq!(store.load().unwrap(), msgs);

        fs::remove_dir_all(&dir).unwrap();
    }
}
