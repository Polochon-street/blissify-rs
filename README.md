Blissify - analyze an MPD library and make smart playlists
==========================================================

Blissify is an MPD plugin for [bliss](https://crates.io/crates/bliss-rs).

You can use it to make playlists of songs that sound alike from an MPD
library.

Usage
=====

Use `cargo install blissify` to install it.

Then analyze your MPD library by using `blissify --update /path/to/mpd/root`
(or `blissify --rescan /path/to/mpd/root`).

Then, when a song is playing, run `blissify --playlist 100` to make a playlist
of 100 songs similar to the current song.
