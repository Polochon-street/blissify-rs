[![crate](https://img.shields.io/crates/v/blissify.svg)](https://crates.io/crates/blissify)
[![build](https://github.com/Polochon-street/blissify-rs/workflows/Rust/badge.svg)](https://github.com/Polochon-street/blissify-rs/actions)

# Blissify - analyze an MPD library and make smart playlists

Blissify is a program used to make playlists of songs that sound alike
from your [MPD](https://www.musicpd.org/) track library, à la Spotify radio.

Under the hood, it is an [MPD](https://www.musicpd.org/) plugin
for [bliss](https://crates.io/crates/bliss-audio).

Blissify needs first to analyze your music library, i.e. compute and store
a series of features from your songs, extracting the tempo, timbre,
loudness, etc.

After that, it is ready to make playlists: play a song to start from, run
`blissify playlist 30`, and voilà! You have a playlist of 30 songs that
sound like your first track.

Note: you *need* to have MPD installed to use blissify. Otherwise, you
probably want to implement bliss-rs support for the audio player you use.

# Installation / Usage

You'll need clang, pkg-config and ffmpeg libraries (including development
headers) to install it, as well as a
[working Rust installation](https://www.rust-lang.org/tools/install)

On Debian-based systems:

    apt install -y clang libavcodec-dev libavformat-dev libavutil-dev pkg-config

On Archlinux:

    pacman -S base-devel clang ffmpeg

Finally, use `cargo install blissify` to install it.


All the commands below read the `MPD_HOST` and `MPD_PORT` environment
variables and try to reach MPD using that. You might want to change
it if MPD is listening to somewhere else than `127.0.0.1:6600` (the default).

## Analyze a library

To initalize and analyze your MPD library, use
```
$ blissify init /path/to/mpd/root
```

Note that it may take several minutes (up to some hours, on very large
libraries with more than for instance 20k songs) to complete.

You can further update your library by running
``` 
$ blissify update
```

If something goes wrong and the database enters an
unstable state, you can use
```
$ blissify rescan
```
to remove the existing database and rescan all files.

If you want to see if the analysis has been successful, or simply want to see
the current files in, you can use
```
$ blissify list-db
```

## Make a playlist

### Simple version

```
$ blissify playlist 100
```

This will add 100 songs similar to the song that is currently
playing on MPD, starting with the closest possible.

### Changing the distance metric

To make a playlist with a distance metric different than the default one
(euclidean distance), which will yield different playlists, run:

```
$ blissify playlist --distance <distance_name> 30
```

`distance_name` is currently `euclidean` and `cosine`. Don't hesitate to
experiment with this parameter if the generated playlists are not to your
linking!

### Make a "seeded" playlist

Instead of making a playlist with songs that are only similar to the first song,
from the most similar to the least similar (the default), you can make a
playlist that queues the closest song to the first song, then the closest song
the second song, etc, effectively making "path" through the songs.

To try it out (it can take a bit more time to build the playlist):
```
$ blissify playlist --seed-song 30
```

### Make an album playlist

You can also make a playlist of album that sound like the current album
your listening to (more specifically, the album of the current song you're
playling, regardless of whether you queued the full album or not).

To try it out:
```
$ blissify playlist --album-playlist 30
```

### Make an interactive playlist

Interactive playlists start from a song, and let you choose which song should
be played next among the 3 closest songs (the number of songs displayed is
can be set manually):

```
$ blissify playlist --interactive-playlist --number-choices 5
```

By default, it crops the current playlist to just keep the currently played
song. If you want to just start from the last song and continue from there, use
`--continue`:

```
$ blissify playlist --interactive-playlist --number-choices 5 --continue
```

# Details

If you are interested about what is happening under the hood, or want to make
a similar plug-in for other audio players, see
[bliss' doc](https://docs.rs/crate/bliss-audio/).
