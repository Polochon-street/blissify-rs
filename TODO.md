# TODO for blissify-rs

This is a todo-list of what's left to do for blissify-rs.
There is quite some work left with regards to distance metrics / way to generate
playlists / MPD integration.

Feel free to submit a PR editing this list if you have some wishes, and to
ask questions if you want to tackle an item.

## Actual TODO

- Add a feature to focus on a specific feature ("I want tempo being prioritized, or timbre, or tonal features")
  => take advantage of the Mahalanobis distance and make it easy-ish to use
- Erroneous report with CUE files (see https://github.com/Polochon-street/blissify-rs/issues/48)
- Try to trim out the crates (it's too big rn)
- grep TODO and see what can be fixed
- Write more tests (run code coverage tools) (maybe integration tests or whatev name is?)
- Split tests and code (module-wise)
- Refactor to merge some things together
- A waypoint feature: go from song1 to song2, both picked by the users, in n songs, without any repetitions between playlist 1 and playlist 2
- A direction feature ("I want the tempo to go down or stay the same")
- A "song group" feature (I want to make a playlist that's in the vibe of these n songs [like 4-5])
- Update clap using the derive feature, and look for clap tests
- DRY tests/cli.rs
- Split up main.rs
- Make sure ffmpeg and symphonia are somehow exclusive (?)

## Done
