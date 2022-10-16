# Changelog

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
