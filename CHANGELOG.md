# Changelog

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
