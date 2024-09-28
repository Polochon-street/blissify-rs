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

    $ sudo apt install -y build-essential pkg-config libavutil-dev libavformat-dev \
    libavfilter-dev libavdevice-dev libclang-dev libsqlite3-dev

On Archlinux:

    $ sudo pacman -S base-devel clang ffmpeg sqlite

Finally, use `cargo install blissify` to install it.

Note: if you are using a raspberry pi and its corresponding ffmpeg
(i.e. `ffmpeg -version|grep rpi` gives back something), use
`cargo install --features=rpi blissify` instead.


All the commands below read the `MPD_HOST` and `MPD_PORT` environment
variables and try to reach MPD using that. You might want to change
it if MPD is listening to somewhere else than `127.0.0.1:6600` (the default).
It should be fully compatible with [the MPD documentation](https://mpd.readthedocs.io/en/latest/client.html#connecting-to-mpd).

## Analyze a library

To initialize and analyze your MPD library, use
```
$ blissify init /path/to/mpd/root
```

It will create a configuration file `config.json` and a database file
`songs.db` in `~/.local/share/bliss-rs`. If you want to specify a different
path for the configuration file and the database file, running
```
$ blissify init -d /path/to/database.db /path/to/mpd/root -c /path/to/configuration.json
```
should do the trick. All the subsequent blissify commands should start
with `blissify <command> -c /path/to/configuration.json` in order to work.

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
the current files in the database, you can use
```
$ blissify list-db
```

## Make a playlist

### Simple version

```
$ blissify playlist 100
```

This will add 100 songs similar to the song that is currently
playing on MPD, starting with the closest possible. This will also remove
all the others songs previously in the queue, leaving only the smart playlist.

If you wish to queue the songs after the current playing song but keep the
current queue, you can use the `--keep-current-queue` flag, like so:

```
$ blissify playlist 100 --keep-current-queue
```

### Changing the distance metric

To make a playlist with a distance metric different than the default one
(euclidean distance), which will yield different playlists, run:

```
$ blissify playlist --distance <distance_name> 30
```

`distance_name` can currently be `euclidean` or `cosine`. Don't hesitate to
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

You can also make a playlist of albums that sound like the current album
you're listening to (more specifically, the album of the current song you're
playing, regardless of whether you queued the full album or not).

To try it out:

```
$ blissify playlist --album-playlist 30
```

If you wish to queue the albums after the current playing album, but keep the
current queue, you can use the `--keep-current-queue` flag, like so:

```
$ blissify playlist --album-playlist 100 --keep-current-queue
```

### Make an interactive playlist

Interactive playlists start from a song, and let you choose which song should
be played next among the 3 closest songs (the number of songs displayed
can be set manually):

```
$ blissify interactive-playlist --number-choices 5
```

By default, it crops the current playlist to just keep the currently played
song. If you want to just start from the last song and continue from there, use
`--continue`:

```
$ blissify interactive-playlist --number-choices 5 --continue
```

### Dry run mode

If you want to see which playlist blissify would make without changing the
queue at all, or you wish to plug blissify's output somewhere else, you
can use the `--dry-run` option, like so:

```
$ blissify playlist 100 --dry-run
```

# Metric learning

If you feel like making your smart™️  playlists even smarter®️ , take a look
at the [metric-learning](https://github.com/Polochon-street/bliss-metric-learning)
repo. It gives you the possibility of evaluating the proximity of your own
songs, and tailor playlists to your own taste.

Once you ran the tool in the [metric-learning](https://github.com/Polochon-street/bliss-metric-learning)
repo, you can use the mahalanobis distance to make playlists from the learned
metric:

```
$ blissify playlist 100 --distance mahalanobis
```

Note that it is all very much alpha development, so if you have any feedback,
feel free to submit an issue.

# Details

If you are interested about what is happening under the hood, or want to make
a similar plug-in for other audio players, see
[bliss' doc](https://docs.rs/crate/bliss-audio/).

# Troubleshooting

If you are compiling blissify-rs for non-linux OSes, you might run into an
error telling you to use the bindgen feature:

```
error: failed to run custom build command for `bliss-audio-aubio-sys v0.2.2`

Caused by:
  process didn't exit successfully: `path/release/build/bliss-audio-aubio-sys-fb4d0ec74b3698ed/build-script-build` (exit status: 101)
  --- stderr
  thread 'main' panicked at .cargo/registry/src/index.crates.io-6f17d22bba15001f/bliss-audio-aubio-sys-0.2.2/build.rs:34:13:
  No prebuilt bindings. Try use `bindgen` feature.
  note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
warning: build failed, waiting for other jobs to finish...
error: failed to compile `blissify v0.4.1`, intermediate artifacts can be found at `/var/folders/pb/g43q604n6v71kwp_89ccy6840000gn/T/cargo-installoyKiUv`.
To reuse those artifacts with a future compilation, set the environment variable `CARGO_TARGET_DIR` to that path.
```

To fix this and build blissify-rs successfully, use `cargo install blissify --features=default,bliss-audio/update-aubio-bindings`.
