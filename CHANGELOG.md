# Changelog

## blissify 0.5.5
* Add a `list-errors` command to list songs that failed the analysis step.

## blissify 0.5.4
* Bump bliss-rs to add the Symphonia decoder.

## blissify 0.5.3
* Bump bliss-rs.

## blissify 0.5.2
* Put the `--config-path` argument after the subcommand for consistency's sake.
* Use XDG_CONFIG_HOME instead of XDG_DATA_HOME to put config files, making
  sure to still check XDG_DATA_HOME for users with an existing database.
* Integration tests can now be run using `cargo test --features=integration-tests`.

## blissify 0.5.1
* Add support for Mahalanobis distance.

## blissify 0.5.0
Note: to take advantage of the fix below, a rescan is needed if you
have an existing database.
* Take into account discs while doing an album playlist.
* Add and store `disc_number` in the database.
* Make `track_number` an integer.

## blissify 0.4.2
* Add a "playlist from playlist" feature (Thanks @SimonTeixidor!)

## blissify 0.4.1
* Make the "--seed-song" option return faster.

## blissify 0.4.0
* Add a "--keep-current-queue" flag that keeps the current queue while making playlists,
  instead of automatically cropping it.
* Stop automatically disabling random mode, and instead warn the user that random is enabled.
* Add a "--dry-run" option.
* Add a "--from-song" option to select a specific song from the command-line.
* Default to deduplicating songs when making playlist; add a "--no-deduplication" option.

## blissify 0.3.12
* Use window / offset to read the list of MPD files to avoid timeout errors.

## blissify 0.3.11
* Bump bliss version.

## blissify 0.3.10
* Fix compilation for non-linux OSes.

## blissify 0.3.9
* Add support for hostnames and abstract sockets in MPD_HOST.

## blissify 0.3.8
* Bump bliss to fix build on raspberry pis.

## blissify 0.3.7
* Bump bliss to get new ffmpeg version and utf-8 fix.

## blissify 0.3.6
* Bump bliss to get rpi feature.

## blissify 0.3.5
* Bump bliss to ensure ffmpeg 6.0 compatibility.
* Rewrite the connectivity code so that MPD_HOST / MPD_PORT work with passwords and
  sockets, in accordance with
  https://mpd.readthedocs.io/en/latest/client.html#connecting-to-mpd.

## blissify 0.3.4
* Bump bliss so updating the database also deletes old song.
  This fixes a bug that would make incomplete playlists when trying to queue
  songs that existed in the database, but no longer in the MPD server.
* Use Rust 2021

## blissify 0.3.3
* Bump bliss to pretty-print json.
* Complete README.
* Fix the init option on `number-cores`.

## blissify 0.3.1
* Add a `number-cores` option.

## blissify 0.3.0
* Use the Library struct.
* Make CUE sheets work.

## blissify 0.2.7
* Add an option to make an interactive playlist.
* Store bliss' features version in the database and use it.

## blissify 0.2.6
* Add an option to make an album playlist.

## blissify 0.2.5
* Complete "mpd_to_bliss" to make the deduplication option work better.

## blissify 0.2.4
* Complete README.
* Explicitely add ffmpeg-next to the list of libs to allow users
  to access ffmpeg-next's flags.

## blissify 0.2.3
* Add the "seed song option".
* Add an option to deduplicate songs in a playlist.

## blissify 0.2.2
* Fix update command to remove songs that were removed from MPD database.

## blissify 0.2.1
* Add a `list-db` subcommand to list what was analyzed.
* Make blissify toggle random mode off when making playlists.
* Make inserts atomic so ctrl+c'd the analysis will not make the next update
  fail.
* Add a proper progressbar for the analysis.

## blissify 0.2.0
* Make blissify subcommands (`blissify update`, etc) instead of flags.
* Change `blissify playlist` to be able to use various distance functions.

## blissify 0.1.8
* Bump bliss version.
* Fix bug that happened when updating an already scanned library with new items.

## blissify 0.1.7

* Use `MPD_HOST` / `MPD_PORT` properly instead of grouping everything into
  a single `MPD_HOST`.
