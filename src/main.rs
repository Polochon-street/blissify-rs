//! Example of how a plugin for an audio player could look like.
//!
//! The handles the analysis of an [MPD](https://www.musicpd.org/) song
//! library, storing songs in an SQLite local database file in
//! ~/.local/share/bliss-rs/songs.db
//!
//! Playlists can then subsequently be made from the current song using
//! --playlist.
use anyhow::{bail, Context, Result};
use bliss_audio::library::{AppConfigTrait, BaseConfig, Library, LibrarySong, ProcessingError};
use bliss_audio::playlist::{
    closest_to_songs, cosine_distance, euclidean_distance, mahalanobis_distance_builder,
    song_to_song, DistanceMetricBuilder,
};
use bliss_audio::{BlissError, BlissResult};
use clap::{App, Arg, ArgMatches, SubCommand};
use log::warn;
use mpd::search::{Query, Term, Window};
use mpd::song::Song as MPDSong;
#[cfg(not(test))]
use mpd::Client;
use noisy_float::prelude::*;
use serde::{Deserialize, Serialize};
use std::char;
#[cfg(not(test))]
use std::env;
#[cfg(not(test))]
use std::net::TcpStream;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use extended_isolation_forest::ForestOptions;

use std::io;
use std::io::Write;
#[cfg(not(test))]
use std::{io::Read, os::unix::net::UnixStream};

use termion::input::TermRead;
use termion::raw::IntoRawMode;

#[cfg(not(feature = "symphonia"))]
use bliss_audio::decoder::ffmpeg::FFmpegDecoder as Decoder;
#[cfg(feature = "symphonia")]
use bliss_audio::decoder::symphonia::SymphoniaDecoder as Decoder;

/// The main struct that stores both the Library object, and some other
/// helper functions to make everything work properly.
struct MPDLibrary {
    // A library object, containing database-related objects.
    pub library: Library<Config, Decoder>,
    /// A connection to the MPD server, used for retrieving song's paths,
    /// currently played songs, and queue tracks.
    #[cfg(not(test))]
    pub mpd_conn: Arc<Mutex<Client<MPDStream>>>,
    /// A mock MPDClient, used for testing purposes only.
    #[cfg(test)]
    pub mpd_conn: Arc<Mutex<MockMPDClient>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Config {
    #[serde(flatten)]
    pub base_config: BaseConfig,
    /// The MPD base path, as specified by the user and written in the MPD
    /// config file. Example: "/home/user/Music".
    pub mpd_base_path: PathBuf,
}

impl Config {
    pub fn new(
        mpd_base_path: PathBuf,
        config_path: Option<PathBuf>,
        database_path: Option<PathBuf>,
        number_cores: Option<NonZeroUsize>,
    ) -> Result<Self> {
        let base_config = BaseConfig::new(config_path, database_path, number_cores)?;
        Ok(Self {
            base_config,
            mpd_base_path,
        })
    }
}

impl AppConfigTrait for Config {
    fn base_config(&self) -> &BaseConfig {
        &self.base_config
    }

    fn base_config_mut(&mut self) -> &mut BaseConfig {
        &mut self.base_config
    }
}

#[cfg(test)]
#[derive(Default)]
/// Convenience Mock for testing.
pub struct MockMPDClient {
    mpd_queue: Vec<MPDSong>,
    // FIXME only useful because the Window struct
    // https://docs.rs/mpd/latest/mpd/search/struct.Window.html
    // is still work in progress, remove when the corresponding
    // fields can be accessed.
    search_window: u32,
}

#[cfg(not(test))]
enum MPDStream {
    Tcp(TcpStream),
    Unix(UnixStream),
}

#[cfg(not(test))]
impl Read for MPDStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            MPDStream::Tcp(v) => v.read(buf),
            MPDStream::Unix(v) => v.read(buf),
        }
    }
}
#[cfg(not(test))]
impl Write for MPDStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            MPDStream::Tcp(v) => v.write(buf),
            MPDStream::Unix(v) => v.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            MPDStream::Tcp(v) => v.flush(),
            MPDStream::Unix(v) => v.flush(),
        }
    }
}

impl MPDLibrary {
    /// Get a connection to the MPD database given some environment
    /// variables.
    #[cfg(not(test))]
    fn get_mpd_conn() -> Result<Client<MPDStream>> {
        #[cfg(target_os = "linux")]
        use std::os::linux::net::SocketAddrExt;
        use std::os::unix::net::SocketAddr;

        let (password, mpd_host) = match env::var("MPD_HOST") {
            Ok(h) => match h.split_once('@') {
                None => (None, h),
                // If it's a unix abstract socket, there will be nothing before the '@'
                Some(("", _)) => (None, h),
                Some((password, host)) => (Some(password.to_owned()), host.to_owned()),
            },
            Err(_) => {
                warn!("Could not find any MPD_HOST environment variable set. Defaulting to 127.0.0.1.");
                (None, String::from("127.0.0.1"))
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

        let mut client = {
            // TODO It is most likely a socket if it starts by "/", but maybe not necessarily?
            // find a solution that doesn't depend on a url crate that pulls the entire internet
            // with it
            if mpd_host.starts_with('/') || mpd_host.starts_with('~') {
                return Ok(Client::new(MPDStream::Unix(UnixStream::connect(
                    mpd_host,
                )?))?);
            }
            #[cfg(target_os = "linux")]
            if mpd_host.starts_with('@') {
                let addr = SocketAddr::from_abstract_name(mpd_host.split_once('@').unwrap().1)?;
                return Ok(Client::new(MPDStream::Unix(UnixStream::connect_addr(
                    &addr,
                )?))?);
            }
            // It is a hostname or an IP address
            Client::new(MPDStream::Tcp(TcpStream::connect(format!(
                "{}:{}",
                mpd_host, mpd_port
            ))?))?
        };
        if let Some(pw) = password {
            client.login(&pw)?;
        }
        Ok(client)
    }

    fn mpd_to_bliss_path(&self, mpd_song: &MPDSong) -> Result<PathBuf> {
        let file = &mpd_song.file;
        let path = if file.to_lowercase().contains(".cue/track")
            || file.to_lowercase().contains(".flac/track")
        {
            let lowercase_string = file.to_lowercase();
            let idx: Vec<_> = lowercase_string.match_indices("/track").collect();
            let beginning_file = file.split_at(idx[0].0).0.to_owned();
            let track_number = file
                .split_at(idx[0].0)
                .1
                .to_owned()
                .strip_prefix("/track")
                .ok_or_else(|| {
                    BlissError::ProviderError(format!(
                        "CUE track {} has an invalid track number",
                        file
                    ))
                })?
                .parse::<usize>()?;
            format!("{}/CUE_TRACK{:03}", beginning_file, track_number)
        } else {
            file.to_string()
        };
        let path = &self.library.config.mpd_base_path.join(PathBuf::from(&path));
        Ok(path.to_path_buf())
    }

    /// Convert a `MPDSong` to a previously analyzed `LibrarySong`, if it exists
    /// in blissify's database.
    fn mpd_to_bliss_song(&self, mpd_song: &MPDSong) -> Result<Option<LibrarySong<()>>> {
        let path = self.mpd_to_bliss_path(mpd_song)?;
        let song = self.library.song_from_path(&path.to_string_lossy()).ok();
        Ok(song)
    }

    /// Convert a bliss song to an MPDSong, regardless whether the song
    /// exists in the MPD database or not.
    ///
    /// Useful to convert CUE tracks to the right format, but does not
    /// include metadata in the MPDSong.
    fn bliss_song_to_mpd(&self, song: &LibrarySong<()>) -> Result<MPDSong> {
        let path = match song.bliss_song.cue_info.to_owned() {
            Some(cue_info) => {
                let track_number = song.bliss_song.track_number.ok_or_else(|| {
                    BlissError::ProviderError(format!(
                        "CUE track {} has an invalid track number",
                        song.bliss_song.path.display()
                    ))
                })?;
                cue_info.cue_path.join(format!("track{:04}", track_number))
            }
            _ => song.bliss_song.path.to_owned(),
        };
        let path = path.strip_prefix(&*self.library.config.mpd_base_path.to_string_lossy())?;
        Ok(MPDSong {
            file: path.to_string_lossy().to_string(),
            ..Default::default()
        })
    }

    /// Create a new MPDLibrary object.
    ///
    /// This means creating the necessary folders and the database file
    /// if it doesn't exist, as well as getting a connection to MPD ready.
    fn new(
        mpd_base_path: PathBuf,
        config_path: Option<PathBuf>,
        database_path: Option<PathBuf>,
        number_cores: Option<NonZeroUsize>,
    ) -> Result<Self> {
        let config = Config::new(mpd_base_path, config_path, database_path, number_cores)?;
        let library = Library::new(config)?;
        let mpd_library = MPDLibrary {
            library,
            mpd_conn: Arc::new(Mutex::new(Self::get_mpd_conn()?)),
        };
        Ok(mpd_library)
    }

    /// Get new MPDLibrary object from an existing configuration.
    ///
    /// This means creating the necessary folders and the database file
    /// if it doesn't exist, as well as getting a connection to MPD ready.
    fn from_config_path(config_path: Option<PathBuf>) -> Result<Self> {
        let library = Library::from_config_path(config_path)?;
        let mpd_library = MPDLibrary {
            library,
            mpd_conn: Arc::new(Mutex::new(Self::get_mpd_conn()?)),
        };
        Ok(mpd_library)
    }

    /// Remove the contents of the current database, and analyze all
    /// MPD's songs again.
    ///
    /// Useful in case the database got corrupted somehow.
    fn full_rescan(&mut self) -> Result<()> {
        let sqlite_conn = self.library.sqlite_conn.lock().unwrap();
        sqlite_conn.execute("delete from feature", [])?;
        sqlite_conn.execute("delete from song", [])?;

        drop(sqlite_conn);
        let paths = self.get_songs_paths()?;
        self.library.analyze_paths(paths, true)?;
        Ok(())
    }

    /// Make a playlist composed of albums similar to the album that's currently playing,
    /// and queue them.
    ///
    /// # Parameters
    ///
    /// - `number_albums`: The number of albums to queue
    /// - `dry_run`: Do not modify the queue, instead print the files that would
    ///   be added to the playlist
    /// - `keep_queue`: if false, will remove the content of the current queue save for the
    ///   currently playing album, and will queue the playlist after the last song of the
    ///   current album. If true, will queue the playlist after the last song of the current album,
    ///   but will keep the queue intact
    // TODO write tests for keep_queue also
    fn queue_from_current_album(
        &self,
        number_albums: usize,
        dry_run: bool,
        keep_queue: bool,
    ) -> Result<()> {
        let mut mpd_conn = self.mpd_conn.lock().unwrap();
        if mpd_conn.status()?.random {
            warn!("Random mode is enabled for MPD, you might want to turn it off to get the most out of your playlist.");
        }
        let mpd_song = match mpd_conn.currentsong()? {
            Some(s) => s,
            None => bail!("No song is currently playing. Add a song to start the playlist from, and try again."),
        };

        let current_song = self.mpd_to_bliss_song(&mpd_song)?.with_context(|| {
            "The song currently playing could not be found in blissify's library. Please analyze it, and try again."
        })?;
        let current_album = current_song.bliss_song.album.ok_or_else(|| {
            BlissError::ProviderError(String::from(
                "The current song does not have any album information.",
            ))
        })?;
        let playlist = self
            .library
            .album_playlist_from::<()>(current_album.clone(), number_albums)?;

        let current_track_number = if let Some(track_number) = &current_song.bliss_song.track_number
        {
            *track_number as usize
        } else {
            1
        };
        // If we don't want to keep the queue, we start the playlist where the
        // currently playing track is playing, and we won't have any album leftovers to
        // shift, since we're erasing the current queue and replacing it with our fresh one.
        let (index, album_leftovers): (usize, usize) = if !keep_queue {
            (current_track_number, 1)
        }
        // If we want to keep the queue, we should iterate on the current playlist
        // until we find the end of the current album, and set the beginning of it there,
        // since we want to preserve the queue as much as possible.
        else {
            let queue_from_current_song = mpd_conn.songs(mpd_song.place.unwrap().pos..)?;
            let album_leftovers = queue_from_current_song
                .iter()
                .take_while(|s| {
                    for (tagname, value) in s.tags.iter() {
                        if tagname.to_ascii_lowercase() == *"album" && *value == current_album {
                            return true;
                        }
                    }
                    false
                })
                .count();
            let index = playlist
                .iter()
                .position(|s| s.bliss_song.album.as_ref() != Some(&current_album))
                .ok_or(BlissError::ProviderError(String::from(
                    "Could not find current album in playlist",
                )))?;
            (index, album_leftovers)
        };

        if dry_run {
            for song in &playlist[index..] {
                println!("{}", song.bliss_song.path.to_string_lossy());
            }
            return Ok(());
        }

        let mut current_pos = mpd_song.place.unwrap().pos;

        // Delete everything except the current song if we don't
        // want to keep the queue.
        if !keep_queue {
            mpd_conn.delete(0..current_pos)?;
            if mpd_conn.queue()?.len() > 1 {
                mpd_conn.delete(1..)?;
            }
            current_pos = 0;
        }
        // Add songs to the queue from the built playlist, starting either
        // from the current song or from the beginning of the next album
        for (i, song) in playlist[index..].iter().enumerate() {
            let mpd_song = self.bliss_song_to_mpd(song)?;
            mpd_conn.insert(mpd_song, (current_pos + i as u32).try_into()?)?;
        }
        let new_pos = current_pos + playlist[index..].len() as u32;
        // Put back the songs from the current album that were shifted around
        mpd_conn.shift(
            new_pos..new_pos + album_leftovers as u32,
            current_pos.try_into()?,
        )?;

        Ok(())
    }

    /// Make a playlist made of songs that are similar to the songs currently
    /// in MPD playlist, and queue these songs after the last one.
    /// Works better with extended_isolation_forest as the distance metric.
    ///
    /// # Parameters
    ///
    /// - `number_songs`: The number of songs to queue.
    /// - `distance`: The distance metric used to compute distances between the current
    ///   playlist and the songs, see [bliss_audio::playlist] for details on distance metrics.
    ///   [extended_isolation_forest] works better than the other for this
    ///   kind of comparison.
    /// - `sort_by`: A closure that does the actual sorting of the playlist in place, based on
    ///   the `distance` metric chosen, see [bliss_audio::playlist::closest_to_songs] for instance
    ///   for details on sorting algorithms.
    /// - `dedup`: Whether or not to deduplicate same songs from the resulting playlist.
    /// - `dry_run`: Do not modify the queue, instead print the files that would
    ///   be added to the playlist.
    fn queue_from_current_playlist<'a, F, I>(
        &self,
        number_songs: usize,
        distance: &'a dyn DistanceMetricBuilder,
        sort_by: F,
        dedup: bool,
        dry_run: bool,
    ) -> Result<()>
    where
        F: Fn(&[LibrarySong<()>], &[LibrarySong<()>], &'a dyn DistanceMetricBuilder) -> I,
        I: Iterator<Item = LibrarySong<()>> + 'a,
    {
        let mut mpd_conn = self.mpd_conn.lock().unwrap();
        if mpd_conn.status()?.random {
            warn!("Random mode is enabled for MPD, you might want to turn it off to get the most out of your playlist.");
        }
        let mpd_songs = mpd_conn.queue()?;

        if mpd_songs.is_empty() {
            bail!("No song is currently playing. Add a song to start the playlist from, and try again.");
        }
        let paths = mpd_songs
            .iter()
            .map(|s| {
                self.mpd_to_bliss_path(s)
                    .map(|s| s.to_string_lossy().to_string())
            })
            .collect::<Result<Vec<String>, _>>()?;
        let paths = paths.iter().map(|s| &**s).collect::<Vec<&str>>();

        let playlist = self
            .library
            .playlist_from_custom(&paths, distance, sort_by, dedup)?
            .take(number_songs);

        if dry_run {
            for song in playlist {
                println!("{}", song.bliss_song.path.to_string_lossy());
            }
            return Ok(());
        }

        for song in playlist {
            let mpd_song = self.bliss_song_to_mpd(&song)?;
            mpd_conn.push(mpd_song)?;
        }
        Ok(())
    }

    /// Make a playlist composed of songs similar to the song that's currently playing,
    /// and queue them.
    ///
    /// # Parameters
    ///
    /// - `song_path`: The path to the song to make a playlist from. Can be either an absolute
    ///   path, i.e. `/home/user/Music/album/song.flac`, or a path relative to
    ///   (mpd_base_path)[Config::mpd_base_path], like `album/song.flac`. If not specified,
    ///   defaults to the currently playing song.
    /// - `number_songs`: The number of songs to queue.
    /// - `distance`: The distance metric used to compute distances between songs, see the
    ///   [bliss_audio::playlist] for details on distance metrics.
    /// - `sort_by`: A closure that does the actual sorting of the playlist in place, based on
    ///   the `distance` metric chosen, see [bliss_audio::playlist::closest_to_songs] for instance
    ///   for details on sorting algorithms.
    /// - `dedup`: Whether or not to deduplicate same songs from the resulting playlist.
    /// - `dry_run`: Do not modify the queue, instead print the files that would
    ///   be added to the playlist.
    /// - `keep_queue`: if false, will remove the content of the entire queue save for the
    ///   currently playing song, and will queue the playlist after it. If true, will queue
    ///   the playlist after the current song, but will keep the queue intact.
    // TODO do we want a flag to toggle "random" off automatically here? And a flag to keep /
    // exclude the current song from the playlist?
    // TODO maybe we don't have to collect? But the magic at the end makes it very convenient
    #[allow(clippy::too_many_arguments)]
    fn queue_from_song<'a, F, I>(
        &self,
        song_path: Option<&str>,
        number_songs: usize,
        distance: &'a dyn DistanceMetricBuilder,
        sort_by: F,
        dedup: bool,
        dry_run: bool,
        keep_queue: bool,
    ) -> Result<()>
    where
        F: Fn(&[LibrarySong<()>], &[LibrarySong<()>], &'a dyn DistanceMetricBuilder) -> I,
        I: Iterator<Item = LibrarySong<()>> + 'a,
    {
        let mut mpd_conn = self.mpd_conn.lock().unwrap();
        if mpd_conn.status()?.random {
            warn!("Random mode is enabled for MPD, you might want to turn it off to get the most out of your playlist.");
        }
        let mpd_song = match mpd_conn.currentsong()? {
            Some(s) => s,
            None => bail!("No song is currently playing. Add a song to start the playlist from, and try again."),
        };
        let path = if let Some(path) = song_path {
            if path.contains(self.library.config.mpd_base_path.to_string_lossy().as_ref()) {
                PathBuf::from(path)
            } else {
                self.library.config.mpd_base_path.join(path)
            }
        } else {
            self.mpd_to_bliss_path(&mpd_song)?
        };

        // If we specified a song path on the CLI, chances are the song is not already
        // in the queue (nor anywhere else).
        // If we didn't, we're using the current_song, and chances are that the song is
        // already in the queue, so we want to get an extra song there, since the current
        // song doesn't count.
        let number_songs = if song_path.is_some() {
            number_songs
        } else {
            number_songs + 1
        };
        let playlist: Vec<LibrarySong<_>> = self
            .library
            .playlist_from_custom(&[&path.to_string_lossy().clone()], distance, sort_by, dedup)?
            .take(number_songs)
            .collect();

        if dry_run {
            for song in &playlist {
                println!("{}", song.bliss_song.path.to_string_lossy());
            }
            return Ok(());
        }

        let mut current_pos = mpd_song.place.unwrap().pos;
        // Delete everything except the current song if we don't
        // want to keep the queue.
        if !keep_queue {
            mpd_conn.delete(0..current_pos)?;
            if mpd_conn.queue()?.len() > 1 {
                mpd_conn.delete(1..)?;
            }
            current_pos = 0;
        }

        // If we're starting from a song specified in song_path,
        // push the playlist straight at the end.
        if song_path.is_some() {
            for song in &playlist {
                let mpd_song = self.bliss_song_to_mpd(song)?;
                mpd_conn.push(mpd_song)?;
            }
            return Ok(());
        }
        // Else, do some magic to preserve the queue depending on the
        // --keep-current-queue argument.
        for (index, song) in playlist[1..].iter().enumerate() {
            let mpd_song = self.bliss_song_to_mpd(song)?;
            mpd_conn.insert(mpd_song, (current_pos + index as u32).try_into()?)?;
        }
        let new_pos = current_pos + playlist.len() as u32 - 1;
        mpd_conn.shift(new_pos..new_pos + 1, current_pos.try_into()?)?;

        Ok(())
    }

    /// Get the song's paths from the MPD database.
    ///
    /// Instead of returning one filename per CUE track (file.cue/track0001,
    /// file2.cue/track0002, etc), returns the CUE sheet itself (file.cue)
    ///
    /// Note: this uses [mpd_base_path](Config::mpd_base_path) because MPD
    /// returns paths without including MPD_BASE_PATH.
    fn get_songs_paths(&self) -> BlissResult<Vec<String>> {
        let mut mpd_conn = self.mpd_conn.lock().unwrap();

        let mut query = Query::new();
        let query = query.and(Term::File, "");
        let (mut index, chunk_size) = (0, 10_000);
        let mut files = vec![];
        loop {
            let search = mpd_conn
                .search(query, Window::from((index, index + chunk_size)))
                .map_err(|e| BlissError::ProviderError(e.to_string()))?;
            if search.is_empty() {
                break;
            }
            files.extend(
                search
                    .into_iter()
                    .map(|s| s.file.to_owned())
                    .map(|s| {
                        if s.to_lowercase().contains(".cue/track") {
                            let lowercase_string = s.to_lowercase();
                            let idx: Vec<_> = lowercase_string.match_indices("/track").collect();
                            s.split_at(idx[0].0).0.to_owned()
                        } else {
                            s
                        }
                    })
                    .map(|s| {
                        String::from(
                            Path::new(&self.library.config.mpd_base_path)
                                .join(Path::new(&s))
                                .to_str()
                                .unwrap(),
                        )
                    })
                    .collect::<Vec<String>>(),
            );
            index += chunk_size;
        }
        files.sort();
        files.dedup();

        Ok(files)
    }

    pub fn make_interactive_playlist(
        &mut self,
        continue_playlist: bool,
        number_choices: usize,
    ) -> Result<()> {
        let mut mpd_conn = self.mpd_conn.lock().unwrap();
        mpd_conn.random(false)?;
        let mpd_song = if !continue_playlist {
            match mpd_conn.currentsong()? {
                Some(s) => s,
                None => bail!(
                    "No song is currently playing. Add a song to start \
                    the playlist from, and try again.",
                ),
            }
        } else {
            match mpd_conn.queue()?.last() {
                Some(s) => s.to_owned(),
                None => bail!(
                    "The current playlist is empty. Add at least a song \
                    to start the playlist from, and try again.",
                ),
            }
        };

        let mut current_song = self.mpd_to_bliss_song(&mpd_song)?.with_context(|| {
            "The current song is not in bliss' database. Run `blissify \
            update /path/to/mpd` and try again."
        })?;
        println!(
            "The playlist will start from: '{} - {}'.",
            current_song
                .bliss_song
                .artist
                .as_deref()
                .unwrap_or("<No artist>"),
            current_song
                .bliss_song
                .title
                .as_deref()
                .unwrap_or("<No title>"),
        );

        // Remove all songs from the playlist except the first one.
        if !continue_playlist {
            let current_pos = mpd_song.place.unwrap().pos;
            mpd_conn.delete(0..current_pos)?;
            if mpd_conn.queue()?.len() > 1 {
                mpd_conn.delete(1..)?;
            }
        }
        let mut songs = self.library.songs_from_library()?;

        let mut playlist = mpd_conn
            .queue()?
            .iter()
            .map(|s| self.mpd_to_bliss_song(s))
            .collect::<Result<Option<Vec<LibrarySong<()>>>>>()?
            .with_context(|| {
                "No song is currently playing. Add a song to start the \
                playlist from, and try again."
            })?;
        songs.retain(|s| !playlist.contains(s));
        println!(
            "The three closest songs will be displayed. Input '1' or 'Enter' \
            to queue the first one, '2' to queue the second one, and '3' \
            for the third one. 'q' or ctrl + c quits the session when you're \
            done.",
        );
        while songs.len() > number_choices {
            if !playlist.is_empty() {
                println!(
                    "Current playlist:\n{}\n",
                    playlist
                        .iter()
                        .map(|s| format!(
                            "\t{} - {}'",
                            s.to_owned()
                                .bliss_song
                                .artist
                                .unwrap_or_else(|| String::from("No artist")),
                            s.to_owned()
                                .bliss_song
                                .title
                                .unwrap_or_else(|| String::from("No title"))
                        ))
                        .collect::<Vec<String>>()
                        .join("\n")
                );
            }
            songs.sort_by_cached_key(|song| {
                n32(euclidean_distance(
                    &current_song.bliss_song.analysis.as_arr1(),
                    &song.bliss_song.analysis.as_arr1(),
                ))
            });
            // TODO put a proper dedup here
            //dedup_playlist(&mut songs, None);
            for (i, song) in songs[1..number_choices + 1].iter().enumerate() {
                println!(
                    "{}: '{} - {}'",
                    i + 1,
                    song.bliss_song
                        .artist
                        .as_ref()
                        .unwrap_or(&String::from("<No artist>")),
                    song.bliss_song
                        .title
                        .as_ref()
                        .unwrap_or(&String::from("<No title>")),
                );
            }

            use std::io::stdin;
            let mut stdout = io::stdout().into_raw_mode().unwrap();
            let stdin = stdin();
            let mut next_song = None;
            let number_choices_digit = char::from_digit(number_choices as u32, 10).unwrap();
            for key in stdin.keys() {
                next_song = if let Ok(key) = key {
                    match key {
                        termion::event::Key::Char('1') | termion::event::Key::Char('\n') => {
                            let mpd_song = self.bliss_song_to_mpd(&songs[1])?;
                            mpd_conn.push(mpd_song)?;
                            let song = songs.remove(1);
                            playlist.push(song.to_owned());
                            Some(song)
                        }
                        termion::event::Key::Char(c @ '2'..='9') if c <= number_choices_digit => {
                            let song = &songs[char::to_digit(c, 10).unwrap() as usize];
                            let mpd_song = self.bliss_song_to_mpd(song)?;
                            mpd_conn.push(mpd_song)?;
                            let song = songs.remove(char::to_digit(c, 10).unwrap() as usize);
                            playlist.push(song.to_owned());
                            Some(song)
                        }
                        termion::event::Key::Char('q') | termion::event::Key::Ctrl('c') => None,
                        _ => continue,
                    }
                } else {
                    continue;
                };
                break;
            }
            if next_song.is_none() {
                break;
            }
            current_song = next_song.unwrap();
            write!(stdout, "{}", termion::clear::All).unwrap();
        }
        Ok(())
    }
}

fn parse_number_cores(matches: &ArgMatches) -> Result<Option<NonZeroUsize>, BlissError> {
    matches
        .value_of("number-cores")
        .map(|x| x.parse::<NonZeroUsize>())
        .map_or(Ok(None), |r| r.map(Some))
        .map_err(|_| BlissError::ProviderError(String::from("Number of cores must be positive")))
}

fn main() -> Result<()> {
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "warn"));
    let config_argument = Arg::with_name("config-path")
            .short("c")
            .long("config-path")
            .help(
                "Optional argument specifying the configuration path, for both loading and initializing blissify. Example: \"/path/to/config.json\". If not specified, defaults to \"XDG_CONFIG_HOME/bliss-rs/config.json\", e.g. \"/home/user/.config/bliss-rs/config.json\".",
            )
            .required(false)
            .takes_value(true);

    let matches = App::new("blissify")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Polochon_street")
        .about("Analyze and make smart playlists from an MPD music database.")
        .arg(config_argument.clone().hidden(true))
        .subcommand(
            SubCommand::with_name("list-db")
            .about("Print songs that have been analyzed and are in blissify's database.")
            .arg(Arg::with_name("detailed").long("detailed")
                .takes_value(false)
                .help("Display analyzed song paths, as well as the corresponding analysis.")
            )
            .arg(config_argument.clone())
        )
        .subcommand(
            SubCommand::with_name("list-errors")
            .arg(config_argument.clone())
            .about("Print songs which could not be analyzed correctly, as well as the associated errors.")
        )
        .subcommand(
            SubCommand::with_name("init")
            .about(
                "Initialize blissify on an MPD library.\n\
                By default, it creates the configuration folder \"bliss-rs\" \
                in XDG_CONFIG_HOME, most likely `/home/user/.config/bliss-rs`, \
                and analyzes the songs in the given MPD library.\n\
                It can take some time."
            )
            .arg(Arg::with_name("MPD_BASE_PATH")
                .help("MPD base path. The value of `music_directory` in your mpd.conf.")
                .required(true)
            )
            .arg(config_argument.clone())
            .arg(Arg::with_name("database-path")
                .short("d")
                .long("database-path")
                .help(
                    "Optional argument specifying where to store the database containing analyzed songs. Example: \"/path/to/bliss.db\". If not specified, defaults to \"XDG_CONFIG_HOME/bliss-rs/songs.db\", e.g. \"/home/user/.config/bliss-rs/songs.db\"."
                )
                .required(false)
                .takes_value(true)
            )
            .arg(Arg::with_name("number-cores")
                .long("number-cores")
                .help(
                    "Number of CPU cores the analysis should use (defaults to the number of cores the CPU has).
Useful to avoid a too heavy load on a machine.")
                .required(false)
                .takes_value(true)
            )
        )
        .subcommand(
            SubCommand::with_name("rescan")
            .arg(config_argument.clone())
            .arg(Arg::with_name("number-cores")
                .long("number-cores")
                .help(
                    "Number of CPU cores the analysis should use (defaults to the number of cores the CPU has).
Useful to avoid a too heavy load on a machine.")
                .required(false)
                .takes_value(true)
            )
            .about("(Re)scan completely an MPD library")
        )
        .subcommand(
            SubCommand::with_name("update")
            .arg(config_argument.clone())
            .arg(Arg::with_name("number-cores")
                .long("number-cores")
                .help(
                    "Number of CPU cores the analysis should use (defaults to the number of cores the CPU has).
Useful to avoid a too heavy load on a machine.")
                .required(false)
                .takes_value(true)
            )
            .about("Scan new songs that were added to the MPD library since last scan.")
        )
        .subcommand(
            SubCommand::with_name("playlist")
            .about("Make a playlist from the currently playing song, clearing the queue and queuing NUMBER_SONGS songs similar to the currently playing song. See the other flags if you want to e.g. preserve the queue.")
            .arg(config_argument.clone())
            .arg(Arg::with_name("NUMBER_SONGS")
                .help("Number of items to queue, including the first song.")
                .required(true)
            )
            .arg(Arg::with_name("distance")
                .long("distance")
                .value_name("distance metric")
                .help(
                    "Choose the distance metric used to make the playlist. Default is 'euclidean' for playlists from a single song, and 'extended_isolation_forest' for playlists from multiple songs. Other options are 'cosine', 'mahalanobis', and 'extended_isolation_forest'. By default, the mahalanobis distance is the same as the euclidean distance. You can tailor this distance to your tastes by running metric learning e.g. using https://github.com/Polochon-street/bliss-metric-learning. The extended_isolation_forest works better for playlists from multiple songs."
                )
                .default_value("euclidean")
            )
            .arg(Arg::with_name("from-song")
                .long("from-song")
                .value_name("song path")
                .help("Instead of making a playlist from the current playing song, make a playlist from 'song path', and add the corresponding songs to the queue. This will also add the song in 'song path' to the playlist.")
            )
            .arg(Arg::with_name("seed")
                .long("seed-song")
                .help(
                    "Instead of making a playlist of only the closest song to the current song, make a playlist that queues the closest song to the first song, then the closest to the second song, etc. Can take some time to build."
                )
                .takes_value(false)
            )
            .arg(Arg::with_name("no-dedup")
                .long("no-deduplication")
                .help(
                    "Do not deduplicate songs based both on the title / artist and their sheer proximity."
                )
                .takes_value(false)
            )
            .arg(Arg::with_name("keep-queue")
                .long("keep-current-queue")
                .help(
                    "Instead of removing the rest of the queue and only keeping the selecting song, queuing songs similiar to the selected song, keep the queue the same, and add similar songs right after the selected song, preserving the rest of the queue."
                )
                .takes_value(false)
            )
            .arg(Arg::with_name("dry-run")
                .long("dry-run")
                .help(
                    "Doesn't actually make any changes to the playlist, but just print songs that would have been added on stdout."
                )
                .takes_value(false)
            )
            .arg(Arg::with_name("album")
                .long("album-playlist")
                .help("Make a playlist of similar albums from the current album.")
                .takes_value(false)
            )
            .arg(Arg::with_name("entire")
                .long("from-entire-playlist")
                .help("Make a playlist of songs similar to all the playlist's songs, \
                    instead of just the first one. Defaults to using the distance metric \
                    extended_isolation_forest, which gives the best results.")
                .takes_value(false)
            )
        )
        .subcommand(
            SubCommand::with_name("interactive-playlist")
            .about(
                "Make a playlist, prompting a set of close songs, and asking which one will be the most appropriate."
            )
            .arg(config_argument.clone())
            .arg(Arg::with_name("continue")
                .long("continue")
                .help(
                    "Take the current playlist's last song as a starting point, instead of removing the current playlist and starting from the first song."
                )
            )
            .arg(Arg::with_name("choices")
                .long("number-choices")
                .value_name("choices")
                .help(
                    "Choose the number of proposed items you get each time.
Defaults to 3, cannot be more than 9."
                )
                .default_value("3")
            )
        )
        .get_matches();

    let mut config_path = match matches.subcommand() {
        (_, Some(sub_m)) => sub_m.value_of("config-path").map(PathBuf::from),
        _ => None,
    };
    if config_path.is_none() {
        config_path = matches.value_of("config-path").map(PathBuf::from);
    }
    if let Some(sub_m) = matches.subcommand_matches("list-db") {
        let library = MPDLibrary::from_config_path(config_path)?;
        let mut songs: Vec<LibrarySong<()>> = library.library.songs_from_library()?;
        songs.sort_by_key(
            |x: &LibrarySong<_>| match x.bliss_song.path.to_str().as_ref() {
                Some(a) => a.to_string(),
                None => String::from(""),
            },
        );
        for song in songs {
            if sub_m.is_present("detailed") {
                println!(
                    "{}: {:?}",
                    song.bliss_song.path.display(),
                    song.bliss_song.analysis
                );
            } else {
                println!("{}", song.bliss_song.path.display());
            }
        }
    } else if matches.subcommand_matches("list-errors").is_some() {
        let library = MPDLibrary::from_config_path(config_path)?;
        let mut failed_songs: Vec<ProcessingError> = library.library.get_failed_songs()?;
        if failed_songs.is_empty() {
            println!("No errors were found from previous analyses.");
        } else {
            failed_songs.sort_by_key(|x| x.song_path.clone());
            for error in &failed_songs {
                println!("{}: {}", error.song_path.display(), error.error);
            }
        }
    } else if let Some(sub_m) = matches.subcommand_matches("init") {
        let database_path = sub_m.value_of("database-path").map(PathBuf::from);
        let number_cores = parse_number_cores(sub_m)?;
        let base_path = sub_m.value_of("MPD_BASE_PATH").unwrap();
        let mut library = MPDLibrary::new(
            PathBuf::from(base_path),
            config_path,
            database_path,
            number_cores,
        )?;

        library.full_rescan()?;
    } else if let Some(sub_m) = matches.subcommand_matches("rescan") {
        let mut library = MPDLibrary::from_config_path(config_path)?;
        let number_cores = parse_number_cores(sub_m)?;
        if let Some(cores) = number_cores {
            library.library.config.set_number_cores(cores)?;
        };
        library.full_rescan()?;
    } else if let Some(sub_m) = matches.subcommand_matches("update") {
        let mut library = MPDLibrary::from_config_path(config_path)?;
        let number_cores = parse_number_cores(sub_m)?;

        if let Some(cores) = number_cores {
            library.library.config.set_number_cores(cores)?;
        };
        let paths = library.get_songs_paths()?;
        library.library.update_library(paths, true, true)?;
    } else if let Some(sub_m) = matches.subcommand_matches("playlist") {
        let number_songs = match sub_m.value_of("NUMBER_SONGS").unwrap().parse::<usize>() {
            Err(_) => {
                bail!("Playlist number must be a valid number.");
            }
            Ok(n) => n,
        };

        let library = MPDLibrary::from_config_path(config_path)?;
        let dry_run = sub_m.is_present("dry-run");
        let no_dedup = sub_m.is_present("no-dedup");
        let keep_queue = sub_m.is_present("keep-queue");

        if sub_m.is_present("album") {
            library.queue_from_current_album(number_songs, dry_run, keep_queue)?;
        } else {
            // TODO let users customize options?
            let forest_distance: &dyn DistanceMetricBuilder = &ForestOptions {
                n_trees: 1000,
                sample_size: 200,
                max_tree_depth: None,
                extension_level: 10,
            };

            let sort = |x: &[LibrarySong<()>],
                        y: &[LibrarySong<()>],
                        z|
             -> Box<dyn Iterator<Item = LibrarySong<()>>> {
                match sub_m.is_present("seed") {
                    false => Box::new(closest_to_songs(x, y, z)),
                    true => Box::new(song_to_song(x, y, z)),
                }
            };
            let distance_metric: &dyn DistanceMetricBuilder = if let Some(m) =
                sub_m.value_of("distance")
            {
                match m {
                    "euclidean" => &euclidean_distance,
                    "cosine" => &cosine_distance,
                    "mahalanobis" => {
                        &mahalanobis_distance_builder(library.library.config.base_config.m.to_owned())
                    }
                    "extended_isolation_forest" => forest_distance,
                    _ => bail!("Please choose a distance name, between 'euclidean', 'cosine', 'mahalanobis' and 'extended_isolation_forest'."),
                }
            } else {
                &euclidean_distance
            };

            if sub_m.is_present("entire") {
                library.queue_from_current_playlist(
                    number_songs,
                    // Defaults to the extended_isolation_forest for multiple songs playlist
                    if sub_m.value_of("distance").is_some() {
                        distance_metric
                    } else {
                        forest_distance
                    },
                    sort,
                    !no_dedup,
                    dry_run,
                )?;
            } else {
                library.queue_from_song(
                    sub_m.value_of("from-song"),
                    number_songs,
                    distance_metric,
                    sort,
                    !no_dedup,
                    dry_run,
                    keep_queue,
                )?;
            }
        }
    } else if let Some(sub_m) = matches.subcommand_matches("interactive-playlist") {
        let number_choices: usize = sub_m.value_of("choices").unwrap_or("3").parse()?;
        let mut library = MPDLibrary::from_config_path(config_path)?;
        if sub_m.is_present("continue") {
            library.make_interactive_playlist(true, number_choices)?;
        } else {
            library.make_interactive_playlist(false, number_choices)?;
        }
    }

    Ok(())
}

// TODO test the playlist length thingy
#[cfg(test)]
mod test {
    use super::*;
    use bliss_audio::{Analysis, Song};
    use mpd::error::Result;
    use mpd::song::{Id, QueuePlace, Song as MPDSong};
    use mpd::Status;
    use pretty_assertions::assert_eq;
    use std::ops;
    use std::time::Duration;
    use tempdir::TempDir;

    impl MockMPDClient {
        pub fn connect(address: &str) -> Result<Self> {
            assert_eq!(address, "127.0.0.1:6600");
            Ok(Self {
                mpd_queue: vec![],
                search_window: 0,
            })
        }

        pub fn currentsong(&mut self) -> Result<Option<MPDSong>> {
            match self.mpd_queue.first() {
                Some(s) => Ok(Some(s.to_owned())),
                None => Ok(None),
            }
        }

        pub fn songs(&mut self, pos: std::ops::RangeFrom<u32>) -> Result<Vec<MPDSong>> {
            let range = std::ops::RangeFrom {
                start: pos.start as usize,
            };
            Ok(self.mpd_queue[range].to_vec())
        }

        pub fn search(&mut self, _: &Query, _: Window) -> Result<Vec<MPDSong>> {
            if self.search_window >= 1 {
                return Ok(vec![]);
            }
            self.search_window += 1;
            Ok(vec![
                MPDSong {
                    file: String::from("s16_mono_22_5kHz.flac"),
                    ..Default::default()
                },
                MPDSong {
                    file: String::from("s16_stereo_22_5kHz.flac"),
                    ..Default::default()
                },
                MPDSong {
                    file: String::from("foo"),
                    ..Default::default()
                },
            ])
        }

        pub fn insert(&mut self, song: MPDSong, pos: usize) -> Result<usize> {
            self.mpd_queue.insert(pos, song);
            Ok(pos)
        }

        pub fn shift(&mut self, from: std::ops::Range<u32>, to: usize) -> Result<()> {
            let value = self.mpd_queue.remove(from.start as usize);
            self.mpd_queue.insert(to, value);
            Ok(())
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

        pub fn status(&mut self) -> Result<Status> {
            Ok(Status {
                random: false,
                ..Default::default()
            })
        }
    }

    impl MPDLibrary {
        pub fn get_mpd_conn() -> Result<MockMPDClient> {
            Ok(MockMPDClient::connect("127.0.0.1:6600").unwrap())
        }
    }

    fn setup_library() -> (MPDLibrary, TempDir) {
        let config_dir = TempDir::new("coucou").unwrap();
        let config_file = config_dir.path().join("config.json");
        let database_file = config_dir.path().join("bliss.db");
        let library = MPDLibrary::new(
            "path".into(),
            Some(config_file),
            Some(database_file),
            Some(NonZeroUsize::new(1).unwrap()),
        )
        .unwrap();
        (library, config_dir)
    }

    #[test]
    fn test_mpd_to_bliss_song() {
        let (library, _tempdir) = setup_library();
        {
            let sqlite_conn = library.library.sqlite_conn.lock().unwrap();
            sqlite_conn
                .execute(
                    "
                insert into song (id, path, title, artist, album, genre, analyzed, version, duration, extra_info) values
                    (1,'path/first_song.flac', 'First Song', 'Art Ist', 'Al Bum', 'Techno', true, 2, 50, null);
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
        let expected_song = LibrarySong {
            extra_info: (),
            bliss_song: Song {
                path: PathBuf::from("path/first_song.flac"),
                title: Some(String::from("First Song")),
                artist: Some(String::from("Art Ist")),
                album: Some(String::from("Al Bum")),
                genre: Some(String::from("Techno")),
                analysis,
                features_version: 2,
                duration: Duration::from_secs(50),
                ..Default::default()
            },
        };
        assert_eq!(song, expected_song);
    }

    #[test]
    fn test_list_errors() {
        let (mut library, _tempdir) = setup_library();
        library.library.config.mpd_base_path = PathBuf::from("data");
        library.full_rescan().unwrap();
        let failed_songs = library.library.get_failed_songs().unwrap();
        if cfg!(feature = "ffmpeg") && cfg!(not(feature = "symphonia")) {
            assert_eq!(
            failed_songs,
            vec![ProcessingError {
                song_path: "data/foo".into(),
                error: "error happened while decoding file - while opening format for file 'data/foo': ffmpeg::Error(2: No such file or directory).".into(),
                features_version: 1,
            }],
        )
        } else if cfg!(feature = "symphonia") {
            assert_eq!(
            failed_songs,
            vec![ProcessingError {
                song_path: "data/foo".into(),
                error: "error happened while decoding file - IO Error: No such file or directory (os error 2)".into(),
                features_version: 1,
            }],
        )
        }
    }

    #[test]
    fn test_playlist_no_song() {
        let (library, _tempdir) = setup_library();

        {
            let sqlite_conn = library.library.sqlite_conn.lock().unwrap();
            sqlite_conn
                .execute(
                    "
                insert into song (id, path, analyzed, duration, version) values
                    (1,'path/first_song.flac', true, 50, 1),
                    (2,'path/second_song.flac', true, 50, 1),
                    (3,'path/last_song.flac', true, 50, 1),
                    (4,'path/unanalyzed.flac', false, 50, 1)
                ",
                    [],
                )
                .unwrap();
        }
        assert_eq!(
            library.queue_from_song(None, 20, &euclidean_distance, closest_to_songs, true, false, false).unwrap_err().to_string(),
            String::from("No song is currently playing. Add a song to start the playlist from, and try again."),
        );
    }

    #[test]
    fn test_playlist_song_not_in_db() {
        let (library, _tempdir) = setup_library();
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

        {
            let sqlite_conn = library.library.sqlite_conn.lock().unwrap();
            sqlite_conn
                .execute(
                    "
                insert into song (id, path, analyzed, version) values
                    (1,'path/first_song.flac', true, 1),
                    (2,'path/second_song.flac', true, 1),
                    (3,'path/last_song.flac', true, 1),
                    (4,'path/unanalyzed.flac', false, 1)
                ",
                    [],
                )
                .unwrap();
        }

        assert_eq!(
            library
                .queue_from_song(
                    None,
                    20,
                    &euclidean_distance,
                    closest_to_songs,
                    true,
                    false,
                    false,
                )
                .unwrap_err()
                .to_string(),
            String::from(
                "error happened with the music library provider - song 'path/not-existing.flac' has not been analyzed",
            ),
        );
    }

    #[test]
    fn test_playlist() {
        let (library, _tempdir) = setup_library();
        library.mpd_conn.lock().unwrap().mpd_queue = vec![
            MPDSong {
                file: String::from("first_song.flac"),
                name: Some(String::from("Coucou")),
                place: Some(QueuePlace {
                    id: Id(1),
                    pos: 0,
                    prio: 0,
                }),
                ..Default::default()
            },
            MPDSong {
                file: String::from("random_song.flac"),
                name: Some(String::from("Coucou")),
                place: Some(QueuePlace {
                    id: Id(1),
                    pos: 1,
                    prio: 0,
                }),
                ..Default::default()
            },
        ];

        // TODO make it better
        {
            let sqlite_conn = library.library.sqlite_conn.lock().unwrap();
            sqlite_conn
                .execute(
                    "
                insert into song (id, path, analyzed, album, track_number, duration, version) values
                    (1,'path/first_song.flac', true, 'Coucou', 1, 10, 1),
                    (2,'path/second_song.flac', true, 'Swag', 1, 20, 1),
                    (3,'path/last_song.flac', true, 'Coucou', 2, 30, 1),
                    (4,'path/unanalyzed.flac', false, null, null, null, 1)
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
        }
        library
            .queue_from_song(
                None,
                20,
                &euclidean_distance,
                closest_to_songs,
                false,
                false,
                false,
            )
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
                String::from("first_song.flac"),
                String::from("second_song.flac"),
                String::from("last_song.flac"),
            ],
        );

        library.mpd_conn.lock().unwrap().mpd_queue = vec![
            MPDSong {
                file: String::from("first_song.flac"),
                name: Some(String::from("Coucou")),
                place: Some(QueuePlace {
                    id: Id(1),
                    pos: 0,
                    prio: 0,
                }),
                ..Default::default()
            },
            MPDSong {
                file: String::from("random_song.flac"),
                name: Some(String::from("Coucou")),
                place: Some(QueuePlace {
                    id: Id(1),
                    pos: 1,
                    prio: 0,
                }),
                ..Default::default()
            },
        ];

        library.queue_from_current_album(20, false, false).unwrap();

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
                String::from("first_song.flac"),
                String::from("last_song.flac"),
                String::from("second_song.flac"),
            ],
        );
    }

    #[test]
    fn test_update() {
        let (mut library, _tempdir) = setup_library();
        library.library.config.mpd_base_path = PathBuf::from("data");
        {
            // TODO do it properly 😩
            let sqlite_conn = library.library.sqlite_conn.lock().unwrap();
            sqlite_conn
                .execute(
                    "
                insert into song (id, path, analyzed, version) values
                    (1, 'data/s16_mono_22_5kHz.flac', true, 1),
                    (10, 'data/coucou.flac', true, 1)
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
        }

        let paths = library.get_songs_paths().unwrap();
        library.library.update_library(paths, true, true).unwrap();

        let sqlite_conn = library.library.sqlite_conn.lock().unwrap();
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
                (String::from("data/foo"), false),
                (String::from("data/s16_mono_22_5kHz.flac"), true),
                (String::from("data/s16_stereo_22_5kHz.flac"), true),
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
        let (mut library, _tempdir) = setup_library();
        library.library.config.mpd_base_path = PathBuf::from("data");

        {
            let sqlite_conn = library.library.sqlite_conn.lock().unwrap();
            // We shouldn't have a song with analyzed = false, but features there,
            // but apparently it can happen, so testing that we recover properly.
            sqlite_conn
                .execute(
                    "
                insert into song (id, path, analyzed, version) values
                    (1, 'data/s16_mono_22_5kHz.flac', false, 1)
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
        }

        let paths = library.get_songs_paths().unwrap();
        library.library.update_library(paths, true, true).unwrap();

        let sqlite_conn = library.library.sqlite_conn.lock().unwrap();
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
                (0, String::from("data/foo"), false),
                (
                    bliss_audio::NUMBER_FEATURES,
                    String::from("data/s16_mono_22_5kHz.flac"),
                    true
                ),
                (
                    bliss_audio::NUMBER_FEATURES,
                    String::from("data/s16_stereo_22_5kHz.flac"),
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
