//! Example of how a plugin for an audio player could look like.
//!
//! The handles the analysis of an [MPD](https://www.musicpd.org/) song
//! library, storing songs in an SQLite local database file in
//! ~/.local/share/bliss-rs/songs.db
//!
//! Playlists can then subsequently be made from the current song using
//! --playlist.
use anyhow::{bail, Context, Result};
use bliss_audio::distance::{
    closest_to_first_song, cosine_distance, dedup_playlist_custom_distance, euclidean_distance,
    song_to_song, DistanceMetric,
};
use bliss_audio::{
    library::analyze_paths_streaming, Analysis, BlissError, BlissResult, Library, Song,
    NUMBER_FEATURES,
};
use clap::{App, Arg, SubCommand};
#[cfg(not(test))]
use dirs::data_local_dir;
use indicatif::{ProgressBar, ProgressStyle};
#[cfg(not(test))]
use log::warn;
use log::{error, info};
use mpd::search::{Query, Term};
use mpd::song::Song as MPDSong;
#[cfg(not(test))]
use mpd::Client;
use rusqlite::{params, Connection, Error as RusqliteError};
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
#[cfg(not(test))]
use std::env;
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The main struct that implements the Library trait, and some other
/// helper functions to make everything work properly.
struct MPDLibrary {
    /// The MPD base path, as specified by the user and written in the MPD
    /// config file.
    pub mpd_base_path: PathBuf,
    /// A connection to blissify's SQLite database, used for storing
    /// and retrieving analyzed songs.
    pub sqlite_conn: Arc<Mutex<Connection>>,
    /// A connection to the MPD server, used for retrieving song's paths,
    /// currently played songs, and queue tracks.
    #[cfg(not(test))]
    pub mpd_conn: Arc<Mutex<Client>>,
    /// A mock MPDClient, used for testing purposes only.
    #[cfg(test)]
    pub mpd_conn: Arc<Mutex<MockMPDClient>>,
}

#[cfg(test)]
#[derive(Default)]
/// Convenience Mock for testing.
pub struct MockMPDClient {
    mpd_queue: Vec<MPDSong>,
}

impl MPDLibrary {
    /// Get a connection to the MPD database given some environment
    /// variables.
    #[cfg(not(test))]
    fn get_mpd_conn() -> Result<Client> {
        let mpd_host = match env::var("MPD_HOST") {
            Ok(h) => h,
            Err(_) => {
                warn!("Could not find any MPD_HOST environment variable set. Defaulting to 127.0.0.1.");
                String::from("127.0.0.1")
            }
        };
        let mpd_port = match env::var("MPD_PORT") {
            Ok(p) => p
                .parse::<u16>()
                .with_context(|| "while trying to coerce MPD_PORT to an integer")?,
            Err(_) => {
                warn!("Could not find any MPD_PORT environment variable set. Defaulting to 6600.");
                6600
            }
        };
        Ok(Client::connect((mpd_host.as_str(), mpd_port))?)
    }

    #[cfg(not(test))]
    fn get_database_folder() -> PathBuf {
        match env::var("XDG_DATA_HOME") {
            Ok(path) => Path::new(&path).join("bliss-rs"),
            Err(_) => data_local_dir().unwrap().join("bliss-rs"),
        }
    }

    /// Convert a `MPDSong` to a previously analyzed `BlissSong`, if it exists
    /// in blissify's database.
    ///
    /// This is done by querying the database for the song's features, and
    /// returns Ok(None) if no such song has been analyzed, Ok(Some(song)) if
    /// a song could be found in blissify's database, and Err(...) if there has
    /// been an error while querying the database.
    ///
    // TODO: this should probably be something with Serialize / Deserialize,
    // but...
    fn mpd_to_bliss_song(&self, mpd_song: &MPDSong) -> Result<Option<Song>> {
        let sql_conn = self.sqlite_conn.lock().unwrap();

        let path = PathBuf::from(&self.mpd_base_path).join(Path::new(&mpd_song.file));
        let mut stmt = sql_conn.prepare(
            "
            select
                feature from feature
                inner join song on song.id = feature.song_id
                where song.path = ? and analyzed = true
                order by song.path, feature.feature_index
            ",
        )?;
        let results = stmt.query_map(params![&path.to_str().unwrap()], |row| row.get(0))?;

        let mut analysis = vec![];
        for result in results {
            analysis.push(result?);
        }
        if analysis.is_empty() {
            bail!("Song '{}' has not been analyzed.", path.display());
        }
        let array: [f32; NUMBER_FEATURES] = analysis.try_into().map_err(|_| {
            BlissError::ProviderError(
                "Too many or too little features were provided at the end of
                the analysis. You might be using an older version of blissify
                with a newer bliss."
                    .to_string(),
            )
        })?;
        let mut stmt = sql_conn.prepare(
            "
            select
                title, artist, album, track_number, genre
                from song where song.path = ? and analyzed = true
                order by song.path;
            ",
        )?;
        let result = stmt.query_row(params![&path.to_str().unwrap()], |row| {
            let title = row.get(0).ok();
            let artist = row.get(1).ok();
            let album = row.get(2).ok();
            let track_number = row.get(3).ok();
            let genre = row.get(4).ok();
            Ok((title, artist, album, track_number, genre))
        })?;

        let song = Song {
            path: path.to_owned(),
            analysis: Analysis::new(array),
            title: result.0,
            artist: result.1,
            album: result.2,
            track_number: result.3,
            genre: result.4,
        };
        Ok(Some(song))
    }

    /// Create a new MPDLibrary object.
    ///
    /// This means creating the necessary folders and the database file
    /// if it doesn't exist, as well as getting a connection to MPD ready.
    fn new(mpd_base_path: String) -> Result<Self> {
        let db_folder = Self::get_database_folder();
        create_dir_all(&db_folder).with_context(|| "While creating config folder")?;
        let db_path = db_folder.join(Path::new("songs.db"));
        let sqlite_conn = Connection::open(db_path)?;
        sqlite_conn.execute(
            "
            create table if not exists song (
                id integer primary key,
                path text not null unique,
                artist text,
                title text,
                album text,
                track_number text,
                genre text,
                stamp timestamp default current_timestamp,
                analyzed boolean default false
            );
            ",
            [],
        )?;
        sqlite_conn.execute("pragma foreign_keys = on;", [])?;
        sqlite_conn.execute(
            "
            create table if not exists feature (
                id integer primary key,
                song_id integer not null,
                feature real not null,
                feature_index integer not null,
                unique(id, feature_index),
                foreign key(song_id) references song(id)
            )
            ",
            [],
        )?;

        Ok(MPDLibrary {
            mpd_base_path: PathBuf::from(mpd_base_path),
            sqlite_conn: Arc::new(Mutex::new(sqlite_conn)),
            mpd_conn: Arc::new(Mutex::new(Self::get_mpd_conn()?)),
        })
    }

    /// Analyze the given paths to various songs, while displaying
    /// progress on the screen with a progressbar.
    fn analyze_paths_showprogress(&mut self, paths: Vec<String>) -> Result<()> {
        let number_songs = paths.len();
        if number_songs == 0 {
            println!("No (new) songs found.");
            return Ok(());
        }
        println!(
            "Analyzing {} songs, this might take some time…",
            number_songs
        );
        let pb = ProgressBar::new(number_songs.try_into().unwrap());
        let style = ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {pos:>7}/{len:7} {wide_msg}")
            .progress_chars("##-");
        pb.set_style(style);

        let results = analyze_paths_streaming(paths)?;
        let mut success_count = 0;
        let mut failure_count = 0;
        for (path, result) in results {
            pb.set_message(format!("Analyzing {}", path));
            match result {
                Ok(song) => {
                    self.store_song(&song)?;
                    success_count += 1;
                }
                Err(e) => {
                    self.store_error_song(path, e)?;
                    failure_count += 1;
                }
            };
            pb.inc(1);
        }
        pb.finish_with_message(format!(
            "Analyzed {} song(s) successfully. {} Failure(s).",
            success_count, failure_count
        ));
        Ok(())
    }

    /// Remove songs in the database identified by their paths.
    ///
    /// Path is without `MPD_BASE_PATH` (so if the song is located at
    /// `/home/foo/Music/artist/album/song.flac` and `MPD_BASE_PATH` is
    /// `/home/foo/Music`, then the path should be `artist/album/song.flac`.
    fn delete(&mut self, to_remove: Vec<String>) -> Result<()> {
        let sqlite_conn = self.sqlite_conn.lock().unwrap();
        let mut count = 0;
        for item in to_remove.iter() {
            sqlite_conn
                .execute(
                    "
                    delete from feature where song_id in (
                        select id from song where path = ?
                    );
                    delete from song where path = ?;
                    ",
                    params![item],
                )
                .map_err(|e| BlissError::ProviderError(e.to_string()))?;
            sqlite_conn
                .execute("delete from song where path = ?;", params![item])
                .map_err(|e| BlissError::ProviderError(e.to_string()))?;
            count += 1;
        }
        info!("Removed {} old songs from blissify's database.", count);
        Ok(())
    }

    /// Update blissify database by analyzing the paths that are listed
    /// by MPD but not currently in the database.
    fn update(&mut self) -> Result<()> {
        let stored_songs = self
            .get_stored_songs()?
            .iter()
            .map(|x| x.path.to_str().unwrap().to_owned())
            .collect::<HashSet<String>>();

        let mpd_songs = {
            let mut mpd_conn = self.mpd_conn.lock().unwrap();
            mpd_conn
                .list(&Term::File, &Query::default())
                .map_err(|e| BlissError::ProviderError(e.to_string()))?
                .into_iter()
                .collect::<HashSet<String>>()
        };

        let to_analyze = mpd_songs
            .difference(&stored_songs)
            .cloned()
            .map(|x| {
                self.mpd_base_path
                    .join(Path::new(&x))
                    .to_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<String>>();
        info!("Found {} new songs to analyze.", to_analyze.len());
        self.analyze_paths_showprogress(to_analyze)?;

        let to_remove = stored_songs
            .difference(&mpd_songs)
            .cloned()
            .collect::<Vec<String>>();
        if to_remove.is_empty() {
            return Ok(());
        }
        info!(
            "Found {} old songs that should be removed from blissify's database.",
            to_remove.len()
        );
        self.delete(to_remove)?;
        Ok(())
    }

    /// Remove the contents of the current database, and analyze all
    /// MPD's songs again.
    ///
    /// Useful in case the database got corrupted somehow.
    fn full_rescan(&mut self) -> Result<()> {
        let sqlite_conn = self.sqlite_conn.lock().unwrap();
        sqlite_conn.execute("delete from feature", [])?;
        sqlite_conn.execute("delete from song", [])?;
        drop(sqlite_conn);
        let paths = self.get_songs_paths()?;
        self.analyze_paths_showprogress(paths)?;
        Ok(())
    }

    fn queue_from_current_song_custom<F, G>(
        &self,
        number_songs: usize,
        distance: G,
        sort: F,
        dedup: bool,
    ) -> Result<()>
    where
        F: FnMut(&Song, &mut Vec<Song>, G),
        G: DistanceMetric + Copy,
    {
        let mut mpd_conn = self.mpd_conn.lock().unwrap();
        mpd_conn.random(false)?;
        let mpd_song = match mpd_conn.currentsong()? {
            Some(s) => s,
            None => bail!("No song is currently playing. Add a song to start the playlist from, and try again."),
        };

        let current_song = self.mpd_to_bliss_song(&mpd_song)?.with_context(|| {
            "No song is currently playing. Add a song to start the playlist from, and try again."
        })?;
        let mut playlist =
            self.playlist_from_song_custom(current_song, number_songs, distance, sort)?;

        if dedup {
            dedup_playlist_custom_distance(&mut playlist, None, distance);
        }
        let current_pos = mpd_song.place.unwrap().pos;
        mpd_conn.delete(0..current_pos)?;
        if mpd_conn.queue()?.len() > 1 {
            mpd_conn.delete(1..)?;
        }

        for song in &playlist[1..] {
            let mpd_song = MPDSong {
                file: song.path.to_string_lossy().to_string(),
                ..Default::default()
            };
            mpd_conn.push(mpd_song)?;
        }
        Ok(())
    }
}

impl Library for MPDLibrary {
    /// Get songs stored in the SQLite database.
    ///
    /// One could also imagine storing songs in a plain JSONlines
    /// or something similar.
    fn get_stored_songs(&self) -> BlissResult<Vec<Song>> {
        let sqlite_conn = self.sqlite_conn.lock().unwrap();
        let mut stmt = sqlite_conn
            .prepare(
                "
                select
                    song.path, feature from feature
                    inner join song on song.id = feature.song_id
                    where song.analyzed = true order by path;
                ",
            )
            .map_err(|e| BlissError::ProviderError(e.to_string()))?;
        let results = stmt
            .query_map([], |row| -> Result<(String, f32), RusqliteError> {
                let path = row.get(0)?;
                let feature = row.get(1)?;
                Ok((path, feature))
            })
            .map_err(|e| BlissError::ProviderError(e.to_string()))?;

        let mut songs_hashmap = HashMap::new();
        for result in results {
            let result = result.map_err(|e| BlissError::ProviderError(e.to_string()))?;
            let song_entry = songs_hashmap.entry(result.0).or_insert_with(Vec::new);
            song_entry.push(result.1);
        }
        songs_hashmap
            .into_iter()
            .map(|(path, analysis)| {
                let array: [f32; NUMBER_FEATURES] = analysis.try_into().map_err(|_| {
                    BlissError::ProviderError(
                        "Too many or too little features were provided at the end of \
                        the analysis. You might be using an older version of blissify \
                        with a newer bliss."
                            .to_string(),
                    )
                })?;

                Ok(Song {
                    path: PathBuf::from(&path),
                    analysis: Analysis::new(array),
                    ..Default::default()
                })
            })
            .collect::<BlissResult<Vec<Song>>>()
    }

    /// Get the song's paths from the MPD database.
    ///
    /// Note: this uses [mpd_base_path](MPDLibrary::mpd_base_path) because MPD
    /// returns paths without including MPD_BASE_PATH.
    fn get_songs_paths(&self) -> BlissResult<Vec<String>> {
        let mut mpd_conn = self.mpd_conn.lock().unwrap();
        Ok(mpd_conn
            .list(&Term::File, &Query::default())
            .map_err(|e| BlissError::ProviderError(e.to_string()))?
            .iter()
            .map(|x| {
                String::from(
                    Path::new(&self.mpd_base_path)
                        .join(Path::new(x))
                        .to_str()
                        .unwrap(),
                )
            })
            .collect::<Vec<String>>())
    }

    /// Store a given [Song](Song) into the SQLite database.
    fn store_song(&mut self, song: &Song) -> Result<(), BlissError> {
        let mut sqlite_conn = self.sqlite_conn.lock().unwrap();
        let path = Path::new(&song.path)
            .strip_prefix(&self.mpd_base_path)
            .unwrap();
        sqlite_conn
            .execute(
                "
            insert into song (
                path, artist, title, album,
                track_number, genre, analyzed
            )
            values (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7
            )
            on conflict(path)
            do update set
                artist=excluded.artist,
                title=excluded.title,
                album=excluded.album,
                track_number=excluded.track_number,
                genre=excluded.genre,
                analyzed=excluded.analyzed
            ",
                params![
                    path.to_str(),
                    song.artist,
                    song.title,
                    song.album,
                    song.track_number,
                    song.genre,
                    true,
                ],
            )
            .map_err(|e| BlissError::ProviderError(e.to_string()))?;
        let last_song_id: i64 = sqlite_conn
            .query_row(
                "select id from song where path = ?1",
                params![path.to_str()],
                |row| row.get(0),
            )
            .map_err(|e| BlissError::ProviderError(e.to_string()))?;
        sqlite_conn
            .execute(
                "delete from feature where song_id = ?1",
                params![last_song_id],
            )
            .map_err(|e| BlissError::ProviderError(e.to_string()))?;

        let tx = sqlite_conn
            .transaction()
            .map_err(|e| BlissError::ProviderError(e.to_string()))?;
        for (index, feature) in song.analysis.as_vec().iter().enumerate() {
            tx.execute(
                "
                insert into feature (song_id, feature, feature_index)
                values (?1, ?2, ?3)
                ",
                params![last_song_id, feature, index],
            )
            .map_err(|e| BlissError::ProviderError(e.to_string()))?;
        }
        tx.commit()
            .map_err(|e| BlissError::ProviderError(e.to_string()))?;
        Ok(())
    }

    /// Store an errored [Song](Song) in the SQLite database.
    ///
    /// Note that it currently doesn't store the actual error; it just stores
    /// the song and sets `analyzed` to `false`.
    fn store_error_song(&mut self, song_path: String, e: BlissError) -> BlissResult<()> {
        let path = song_path.strip_prefix(&self.mpd_base_path.to_str().unwrap());
        self.sqlite_conn
            .lock()
            .unwrap()
            .execute(
                "
            insert or ignore into song(path) values (?1)
            ",
                [path],
            )
            .map_err(|e| BlissError::ProviderError(e.to_string()))?;
        error!(
            "Analysis of song '{}' failed: {} The error has been stored.",
            song_path, e
        );
        Ok(())
    }
}

fn main() -> Result<()> {
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "warn"));

    let matches = App::new("blissify")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Polochon_street")
        .about("Analyze and make smart playlists from an MPD music database.")
        .subcommand(
            SubCommand::with_name("list-db")
            .about("Print songs that have been analyzed and are in blissify's database.")
            .arg(Arg::with_name("detailed").long("detailed")
                .takes_value(false)
                .help("Display analyzed song paths, as well as the corresponding analysis.")
            )
        )
        .subcommand(
            SubCommand::with_name("rescan")
            .about("(Re)scan completely an MPD library")
            .arg(Arg::with_name("MPD_BASE_PATH")
                .help("MPD base path. The value of `music_directory` in your mpd.conf.")
                .required(true)
            )
        )
        .subcommand(
            SubCommand::with_name("update")
            .about("Scan new songs that were added to the MPD library since last scan.")
            .arg(Arg::with_name("MPD_BASE_PATH")
                .help("MPD base path. The value of `music_directory` in your mpd.conf.")
                .required(true)
            )
        )
        .subcommand(
            SubCommand::with_name("playlist")
            .about("Erase the current playlist and make playlist of PLAYLIST_LENGTH from the currently played song")
            .arg(Arg::with_name("PLAYLIST_LENGTH")
                .help("The desired playlist length, including the first song.")
                .required(true)
            )
            .arg(Arg::with_name("distance")
                .long("distance")
                .value_name("distance metric")
                .help(
                    "Choose the distance metric used to make the playlist. Default is 'euclidean',\
                    other option is 'cosine'"
                )
                .default_value("euclidean")
            )
            .arg(Arg::with_name("seed")
                .long("seed-song")
                .help(
                    "Instead of making a playlist of only the closest song to the current song,\
                    make a playlist that queues the closest song to the first song, then
                    the closest to the second song, etc. Can take some time to build."
                )
                .takes_value(false)
            )
            .arg(Arg::with_name("dedup")
                .long("deduplicate-songs")
                .help(
                    "Deduplicate songs based both on the title / artist and their\
                     sheer proximity."
                )
                .takes_value(false)
            )
        )
        .get_matches();

    if let Some(sub_m) = matches.subcommand_matches("list-db") {
        let library = MPDLibrary::new(String::from(""))?;
        let mut songs = library.get_stored_songs()?;

        songs.sort_by_key(|x| match x.path.to_str().as_ref() {
            Some(a) => a.to_string(),
            None => String::from(""),
        });
        for song in songs {
            if sub_m.is_present("detailed") {
                println!("{}: {:?}", song.path.display(), song.analysis);
            } else {
                println!("{}", song.path.display());
            }
        }
    } else if let Some(sub_m) = matches.subcommand_matches("rescan") {
        let base_path = sub_m.value_of("MPD_BASE_PATH").unwrap();
        let mut library = MPDLibrary::new(base_path.to_string())?;
        library.full_rescan()?;
    } else if let Some(sub_m) = matches.subcommand_matches("update") {
        let base_path = sub_m.value_of("MPD_BASE_PATH").unwrap();
        let mut library = MPDLibrary::new(base_path.to_string())?;
        library.update()?;
    } else if let Some(sub_m) = matches.subcommand_matches("playlist") {
        let number_songs = match sub_m.value_of("PLAYLIST_LENGTH").unwrap().parse::<usize>() {
            Err(_) => {
                bail!("Playlist number must be a valid number.");
            }
            Ok(n) => n,
        };

        let distance_metric = if let Some(m) = sub_m.value_of("distance") {
            match m {
                "euclidean" => euclidean_distance,
                "cosine" => cosine_distance,
                _ => bail!("Please choose a distance name, between 'euclidean' and 'cosine'."),
            }
        } else {
            euclidean_distance
        };

        let library = MPDLibrary::new(String::from(""))?;
        let sort = match sub_m.is_present("seed") {
            false => closest_to_first_song,
            true => song_to_song,
        };
        if sub_m.is_present("dedup") {
            library.queue_from_current_song_custom(number_songs, distance_metric, sort, true)?;
        } else {
            library.queue_from_current_song_custom(number_songs, distance_metric, sort, false)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use mpd::error::Result;
    use mpd::song::{Id, QueuePlace, Song as MPDSong};
    use std::ops;
    use tempdir::TempDir;

    impl MockMPDClient {
        pub fn connect(address: &str) -> Result<Self> {
            assert_eq!(address, "127.0.0.1:6600");
            Ok(Self { mpd_queue: vec![] })
        }

        pub fn currentsong(&mut self) -> Result<Option<MPDSong>> {
            match self.mpd_queue.first() {
                Some(s) => Ok(Some(s.to_owned())),
                None => Ok(None),
            }
        }

        pub fn list(&mut self, term: &Term, _: &Query) -> Result<Vec<String>> {
            assert!(matches!(term, Term::File));
            Ok(vec![
                String::from("s16_mono_22_5kHz.flac"),
                String::from("s16_stereo_22_5kHz.flac"),
                String::from("foo"),
            ])
        }

        pub fn queue(&mut self) -> Result<Vec<MPDSong>> {
            Ok(self.mpd_queue.to_owned())
        }

        pub fn delete<T>(&mut self, range: T) -> Result<()>
        where
            T: ops::RangeBounds<u32> + Iterator<Item = u32>,
        {
            // poor man's range
            for i in range {
                if i > self.mpd_queue.len() as u32 {
                    break;
                }
                self.mpd_queue.remove(i as usize);
            }
            Ok(())
        }

        pub fn push(&mut self, song: MPDSong) -> Result<()> {
            self.mpd_queue.push(song);
            Ok(())
        }

        pub fn random(&mut self, state: bool) -> Result<()> {
            assert!(!state);
            Ok(())
        }
    }

    impl MPDLibrary {
        pub fn get_mpd_conn() -> Result<MockMPDClient> {
            Ok(MockMPDClient::connect("127.0.0.1:6600").unwrap())
        }

        pub fn get_database_folder() -> PathBuf {
            TempDir::new("test").unwrap().path().to_path_buf()
        }
    }

    #[test]
    fn test_mpd_to_bliss_song() {
        let library = MPDLibrary::new(String::from("path/")).unwrap();

        {
            let sqlite_conn = library.sqlite_conn.lock().unwrap();
            sqlite_conn
                .execute(
                    "
                insert into song (id, path, title, artist, album, genre, analyzed) values
                    (1,'path/first_song.flac', 'First Song', 'Art Ist', 'Al Bum', 'Techno', true);
                ",
                    [],
                )
                .unwrap();

            sqlite_conn
                .execute(
                    "
                insert into feature (song_id, feature, feature_index) values
                    (1, 0., 1),
                    (1, 0., 2),
                    (1, 0., 3),
                    (1, 0., 4),
                    (1, 0., 5),
                    (1, 0., 6),
                    (1, 0., 7),
                    (1, 0., 8),
                    (1, 0., 9),
                    (1, 0., 10),
                    (1, 0., 11),
                    (1, 0., 12),
                    (1, 0., 13),
                    (1, 0., 14),
                    (1, 0., 15),
                    (1, 0., 16),
                    (1, 0., 17),
                    (1, 0., 18),
                    (1, 0., 19),
                    (1, 0.3, 20);
                 ",
                    [],
                )
                .unwrap();
        }
        let mpd_song = MPDSong {
            file: String::from("first_song.flac"),
            name: Some(String::from("First Song")),
            place: Some(QueuePlace {
                id: Id(1),
                pos: 50,
                prio: 0,
            }),
            ..Default::default()
        };
        let analysis = Analysis::new([
            0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.3,
        ]);
        let song = library.mpd_to_bliss_song(&mpd_song).unwrap().unwrap();
        let expected_song = Song {
            path: PathBuf::from("path/first_song.flac"),
            title: Some(String::from("First Song")),
            artist: Some(String::from("Art Ist")),
            album: Some(String::from("Al Bum")),
            genre: Some(String::from("Techno")),
            analysis: analysis,
            ..Default::default()
        };
        assert_eq!(song, expected_song);
    }

    #[test]
    fn test_playlist_no_song() {
        let library = MPDLibrary::new(String::from("")).unwrap();

        let sqlite_conn = library.sqlite_conn.lock().unwrap();
        sqlite_conn
            .execute(
                "
            insert into song (id, path, analyzed) values
                (1,'path/first_song.flac', true),
                (2,'path/second_song.flac', true),
                (3,'path/last_song.flac', true),
                (4,'path/unanalyzed.flac', false)
            ",
                [],
            )
            .unwrap();

        drop(sqlite_conn);
        assert_eq!(
            library.queue_from_current_song_custom(20, euclidean_distance, closest_to_first_song, true).unwrap_err().to_string(),
            String::from("No song is currently playing. Add a song to start the playlist from, and try again."),
        );
    }

    #[test]
    fn test_playlist_song_not_in_db() {
        let library = MPDLibrary::new(String::from("")).unwrap();
        library.mpd_conn.lock().unwrap().mpd_queue = vec![MPDSong {
            file: String::from("not-existing.flac"),
            name: Some(String::from("Coucou")),
            place: Some(QueuePlace {
                id: Id(1),
                pos: 50,
                prio: 0,
            }),
            ..Default::default()
        }];

        let sqlite_conn = library.sqlite_conn.lock().unwrap();
        sqlite_conn
            .execute(
                "
            insert into song (id, path, analyzed) values
                (1,'path/first_song.flac', true),
                (2,'path/second_song.flac', true),
                (3,'path/last_song.flac', true),
                (4,'path/unanalyzed.flac', false)
            ",
                [],
            )
            .unwrap();

        drop(sqlite_conn);
        assert_eq!(
            library
                .queue_from_current_song_custom(20, euclidean_distance, closest_to_first_song, true)
                .unwrap_err()
                .to_string(),
            String::from("Song 'not-existing.flac' has not been analyzed."),
        );
    }

    #[test]
    fn test_playlist() {
        let library = MPDLibrary::new(String::from("")).unwrap();
        library.mpd_conn.lock().unwrap().mpd_queue = vec![
            MPDSong {
                file: String::from("path/first_song.flac"),
                name: Some(String::from("Coucou")),
                place: Some(QueuePlace {
                    id: Id(1),
                    pos: 0,
                    prio: 0,
                }),
                ..Default::default()
            },
            MPDSong {
                file: String::from("path/random_song.flac"),
                name: Some(String::from("Coucou")),
                place: Some(QueuePlace {
                    id: Id(1),
                    pos: 1,
                    prio: 0,
                }),
                ..Default::default()
            },
        ];

        let sqlite_conn = library.sqlite_conn.lock().unwrap();
        sqlite_conn
            .execute(
                "
            insert into song (id, path, analyzed) values
                (1,'path/first_song.flac', true),
                (2,'path/second_song.flac', true),
                (3,'path/last_song.flac', true),
                (4,'path/unanalyzed.flac', false)
            ",
                [],
            )
            .unwrap();

        sqlite_conn
            .execute(
                "
            insert into feature (song_id, feature, feature_index) values
                (1, 0., 1),
                (1, 0., 2),
                (1, 0., 3),
                (1, 0., 4),
                (1, 0., 5),
                (1, 0., 6),
                (1, 0., 7),
                (1, 0., 8),
                (1, 0., 9),
                (1, 0., 10),
                (1, 0., 11),
                (1, 0., 12),
                (1, 0., 13),
                (1, 0., 14),
                (1, 0., 15),
                (1, 0., 16),
                (1, 0., 17),
                (1, 0., 18),
                (1, 0., 19),
                (1, 0., 20),
                (2, 0.1, 1),
                (2, 0.1, 2),
                (2, 0.1, 3),
                (2, 0.1, 4),
                (2, 0.1, 5),
                (2, 0.1, 6),
                (2, 0.1, 7),
                (2, 0.1, 8),
                (2, 0.1, 9),
                (2, 0.1, 10),
                (2, 0.1, 11),
                (2, 0.1, 12),
                (2, 0.1, 13),
                (2, 0.1, 14),
                (2, 0.1, 15),
                (2, 0.1, 16),
                (2, 0.1, 17),
                (2, 0.1, 18),
                (2, 0.1, 19),
                (2, 0.1, 20),
                (3, 10, 1),
                (3, 10, 2),
                (3, 10, 3),
                (3, 10, 4),
                (3, 10, 5),
                (3, 10, 6),
                (3, 10, 7),
                (3, 10, 8),
                (3, 10, 9),
                (3, 10, 10),
                (3, 10, 11),
                (3, 10, 12),
                (3, 10, 13),
                (3, 10, 14),
                (3, 10, 15),
                (3, 10, 16),
                (3, 10, 17),
                (3, 10, 18),
                (3, 10, 19),
                (3, 10, 20);
            ",
                [],
            )
            .unwrap();
        drop(sqlite_conn);
        library
            .queue_from_current_song_custom(20, euclidean_distance, closest_to_first_song, false)
            .unwrap();

        let playlist = library
            .mpd_conn
            .lock()
            .unwrap()
            .mpd_queue
            .iter()
            .map(|x| x.file.to_owned())
            .collect::<Vec<String>>();

        assert_eq!(
            playlist,
            vec![
                String::from("path/first_song.flac"),
                String::from("path/second_song.flac"),
                String::from("path/last_song.flac"),
            ],
        );
    }

    #[test]
    fn test_update() {
        let mut library = MPDLibrary::new(String::from("./data/")).unwrap();

        let sqlite_conn = library.sqlite_conn.lock().unwrap();
        sqlite_conn
            .execute(
                "
            insert into song (id, path, analyzed) values
                (1, 's16_mono_22_5kHz.flac', true),
                (10, 'coucou.flac', true)
            ",
                [],
            )
            .unwrap();

        let mut sqlite_string =
            String::from("insert into feature (song_id, feature, feature_index) values\n");
        sqlite_string.push_str(
            &(0..20)
                .into_iter()
                .map(|i| String::from(&format!("(1, 0., {})", i)))
                .collect::<Vec<String>>()
                .join(",\n"),
        );
        sqlite_string.push_str(",\n");
        sqlite_string.push_str(
            &(0..20)
                .into_iter()
                .map(|i| String::from(&format!("(10, 0., {})", i)))
                .collect::<Vec<String>>()
                .join(",\n"),
        );
        sqlite_conn.execute(&sqlite_string, []).unwrap();
        drop(sqlite_conn);

        library.update().unwrap();

        let sqlite_conn = library.sqlite_conn.lock().unwrap();
        let mut stmt = sqlite_conn
            .prepare("select path, analyzed from song order by path")
            .unwrap();
        let expected_songs = stmt
            .query_map([], |row| Ok((row.get(0).unwrap(), row.get(1).unwrap())))
            .unwrap()
            .map(|x| {
                let x = x.unwrap();
                (x.0, x.1)
            })
            .collect::<Vec<(String, bool)>>();

        assert_eq!(
            expected_songs,
            vec![
                (String::from("foo"), false),
                (String::from("s16_mono_22_5kHz.flac"), true),
                (String::from("s16_stereo_22_5kHz.flac"), true),
            ],
        );

        let mut stmt = sqlite_conn
            .prepare("select count(*) from feature group by song_id")
            .unwrap();
        let expected_feature_count = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|x| x.unwrap())
            .collect::<Vec<u32>>();
        for feature_count in expected_feature_count {
            assert!(feature_count > 1);
        }
    }

    #[test]
    fn test_update_screwed_db() {
        let mut library = MPDLibrary::new(String::from("./data/")).unwrap();

        let sqlite_conn = library.sqlite_conn.lock().unwrap();
        // We shouldn't have a song with analyzed = false, but features there,
        // but apparently it can happen, so testing that we recover properly.
        sqlite_conn
            .execute(
                "
            insert into song (id, path, analyzed) values
                (1, 's16_mono_22_5kHz.flac', false)
            ",
                [],
            )
            .unwrap();

        sqlite_conn
            .execute(
                "
            insert into feature (song_id, feature, feature_index) values
                (1, 0., 1),
                (1, 0., 2),
                (1, 0., 3),
                (1, 0., 4),
                (1, 0., 5),
                (1, 0., 6),
                (1, 0., 7),
                (1, 0., 8),
                (1, 0., 9),
                (1, 0., 10),
                (1, 0., 11),
                (1, 0., 12),
                (1, 0., 13),
                (1, 0., 14),
                (1, 0., 15),
                (1, 0., 16),
                (1, 0., 17),
                (1, 0., 18),
                (1, 0., 19),
                (1, 0., 20);
            ",
                [],
            )
            .unwrap();
        drop(sqlite_conn);

        library.update().unwrap();

        let sqlite_conn = library.sqlite_conn.lock().unwrap();
        let mut stmt = sqlite_conn
            .prepare("select count(song_id), path, analyzed from song left outer join feature on feature.song_id = song.id group by song.id order by path")
            .unwrap();
        let expected_songs = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0).unwrap(),
                    row.get(1).unwrap(),
                    row.get(2).unwrap(),
                ))
            })
            .unwrap()
            .map(|x| {
                let x = x.unwrap();
                (x.0, x.1, x.2)
            })
            .collect::<Vec<(usize, String, bool)>>();

        assert_eq!(
            expected_songs,
            vec![
                (0, String::from("foo"), false),
                (NUMBER_FEATURES, String::from("s16_mono_22_5kHz.flac"), true),
                (
                    NUMBER_FEATURES,
                    String::from("s16_stereo_22_5kHz.flac"),
                    true
                ),
            ],
        );

        let mut stmt = sqlite_conn
            .prepare("select count(*) from feature group by song_id")
            .unwrap();
        let expected_feature_count = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|x| x.unwrap())
            .collect::<Vec<u32>>();
        for feature_count in expected_feature_count {
            assert!(feature_count > 1);
        }
    }
}
