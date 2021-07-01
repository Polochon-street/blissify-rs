//! Example of how a plugin for an audio player could look like.
//!
//! The handles the analysis of an [MPD](https://www.musicpd.org/) song
//! library, storing songs in an SQLite local database file in
//! ~/.local/share/bliss-rs/songs.db
//!
//! Playlists can then subsequently be made from the current song using
//! --playlist.
use anyhow::{bail, Context, Result};
use bliss_audio::distance::{cosine_distance, euclidean_distance, DistanceMetric};
use bliss_audio::{Analysis, BlissError, BlissResult, Library, Song, NUMBER_FEATURES};
use clap::{App, Arg, SubCommand};
#[cfg(not(test))]
use dirs::data_local_dir;
use log::info;
#[cfg(not(test))]
use log::warn;
use mpd::search::{Query, Term};
use mpd::song::Song as MPDSong;
#[cfg(not(test))]
use mpd::Client;
use rusqlite::{params, Connection, Error as RusqliteError, Row};
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
#[cfg(not(test))]
use std::env;
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct MPDLibrary {
    pub mpd_base_path: PathBuf,
    pub sqlite_conn: Arc<Mutex<Connection>>,
    #[cfg(not(test))]
    pub mpd_conn: Arc<Mutex<Client>>,
    #[cfg(test)]
    pub mpd_conn: Arc<Mutex<MockMPDClient>>,
}

#[cfg(test)]
#[derive(Default)]
pub struct MockMPDClient {
    mpd_queue: Vec<MPDSong>,
}

impl MPDLibrary {
    fn row_closure(row: &Row) -> Result<(String, f32), RusqliteError> {
        let path = row.get(0)?;
        let feature = row.get(1)?;
        Ok((path, feature))
    }

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

    fn mpd_to_bliss_song(&self, mpd_song: &MPDSong) -> Result<Option<Song>> {
        let sql_conn = self.sqlite_conn.lock().unwrap();

        let path = PathBuf::from(&self.mpd_base_path).join(Path::new(&mpd_song.file));
        let mut stmt = sql_conn.prepare(
            "
            select
                song.path, feature from feature
                inner join song on song.id = feature.song_id
                where song.path = ? and analyzed = true
                order by song.path, feature.feature_index
            ",
        )?;
        let results = stmt.query_map(params![&path.to_str().unwrap()], MPDLibrary::row_closure)?;

        let mut song = Song {
            path: path.to_owned(),
            ..Default::default()
        };
        let mut analysis = vec![];
        for result in results {
            analysis.push(result?.1);
        }
        if analysis.is_empty() {
            bail!("Song '{}' has not been analyzed.", song.path.display());
        }
        let array: [f32; NUMBER_FEATURES] = analysis.try_into().map_err(|_| {
            BlissError::ProviderError(
                "Too many or too little features were provided at the end of
                the analysis. You might be using an older version of blissify
                with a newer bliss."
                    .to_string(),
            )
        })?;
        song.analysis = Analysis::new(array);
        Ok(Some(song))
    }

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

    fn update(&mut self) -> Result<()> {
        let stored_songs = self
            .get_stored_songs()?
            .iter()
            .map(|x| {
                self.mpd_base_path
                    .join(Path::new(&x.path.to_owned()))
                    .to_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<HashSet<String>>();
        let mpd_songs = self
            .get_songs_paths()?
            .into_iter()
            .collect::<HashSet<String>>();
        let to_analyze = mpd_songs
            .difference(&stored_songs)
            .cloned()
            .collect::<Vec<String>>();
        info!("Found {} new songs to analyze.", to_analyze.len());
        self.analyze_paths(to_analyze)?;
        Ok(())
    }

    fn full_rescan(&mut self) -> Result<()> {
        let sqlite_conn = self.sqlite_conn.lock().unwrap();
        sqlite_conn.execute("delete from feature", [])?;
        sqlite_conn.execute("delete from song", [])?;
        drop(sqlite_conn);
        self.analyze_library()?;
        Ok(())
    }

    fn queue_from_current_song_custom_distance(
        &self,
        number_songs: usize,
        distance: impl DistanceMetric,
    ) -> Result<()> {
        let mut mpd_conn = self.mpd_conn.lock().unwrap();
        let mpd_song = match mpd_conn.currentsong()? {
            Some(s) => s,
            None => bail!("No song is currently playing. Add a song to start the playlist from, and try again."),
        };

        let current_song = self.mpd_to_bliss_song(&mpd_song)?.with_context(|| {
            "No song is currently playing. Add a song to start the playlist from, and try again."
        })?;
        let playlist =
            self.playlist_from_song_custom_distance(current_song, number_songs, distance)?;

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
            .query_map([], MPDLibrary::row_closure)
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
                        "Too many or too little features were provided at the end of    
                        the analysis. You might be using an older version of blissify
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

    fn store_song(&mut self, song: &Song) -> Result<(), BlissError> {
        let sqlite_conn = self.sqlite_conn.lock().unwrap();
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

        for (index, feature) in song.analysis.to_vec().iter().enumerate() {
            sqlite_conn
                .execute(
                    "
                insert into feature (song_id, feature, feature_index)
                values (?1, ?2, ?3)
                ",
                    params![last_song_id, feature, index],
                )
                .map_err(|e| BlissError::ProviderError(e.to_string()))?;
        }
        Ok(())
    }

    fn store_error_song(&mut self, song_path: String, _: BlissError) -> BlissResult<()> {
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
        Ok(())
    }
}

fn main() -> Result<()> {
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "info"));

    let matches = App::new("blissify")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Polochon_street")
        .about("Analyze a MPD music database, and make playlists.")
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
            .about("Build a playlist from the currently played song")
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
        )
        .get_matches();

    if let Some(sub_m) = matches.subcommand_matches("rescan") {
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
        library.queue_from_current_song_custom_distance(number_songs, distance_metric)?;
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
            library.queue_from_current_song_custom_distance(20, euclidean_distance).unwrap_err().to_string(),
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
                .queue_from_current_song_custom_distance(20, euclidean_distance)
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
            .queue_from_current_song_custom_distance(20, euclidean_distance)
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
                (1, 's16_mono_22_5kHz.flac', true)
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
