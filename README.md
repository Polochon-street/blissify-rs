[![crate](https://img.shields.io/crates/v/blissify.svg)](https://crates.io/crates/blissify)
[![build](https://github.com/Polochon-street/blissify-rs/workflows/Rust/badge.svg)](https://github.com/Polochon-street/blissify-rs/actions)

Blissify - analyze an MPD library and make smart playlists
==========================================================

Blissify is a program used to make playlists of songs that sound alike
from your [MPD](https://www.musicpd.org/) track library, à la Spotify radio.

Under the hood, it is an [MPD](https://www.musicpd.org/) plugin
for [bliss](https://crates.io/crates/bliss-audio).

Blissify needs first to analyze your music library, i.e. compute and store
a series of features from your songs, extracting the tempo, timbre,
loudness, etc.

After that, it is ready to make playlists: play a song to start from, run
`blissify --playlist 30`, and voilà! You have a playlist of 30 songs that
sound like your first track.

Usage
=====

Use `cargo install blissify` to install it.

Analyze a library
-----------------

To analyze your MPD library, use
```
$ blissify --update /path/to/mpd/root
```
(or `blissify --rescan /path/to/mpd/root`).

Make a playlist
---------------

```
$ blissify --playlist 100
```

This will add 100 songs similar to the song that is currently
playing on MPD, starting with the closest possible.

Details
=======

If you are interested about what is happening under the hood, or want to make
a similar plug-in for other audio players, see
[bliss' doc](https://docs.rs/crate/bliss-audio/0.1.3).
