[![crate](https://img.shields.io/crates/v/blissify.svg)](https://crates.io/crates/blissify)
[![build](https://github.com/Polochon-street/blissify-rs/workflows/Rust/badge.svg)](https://github.com/Polochon-street/blissify-rs/actions)

Blissify - analyze an MPD library and make smart playlists
==========================================================

Blissify is an [MPD](https://www.musicpd.org/) plugin
for [bliss](https://crates.io/crates/bliss-audio).

You can use it to make playlists of songs that sound alike from an MPD
library.

Note: the `blissify-rs` crate is outdated. Use this crate (`blissify`) instead.

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
