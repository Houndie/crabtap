# Crabtap
_A tui for generating tap bpms in rust_

![example](https://raw.githubusercontent.com/Houndie/crabtap/master/crabtap.gif)

## About

I made Crabtap in an effort to generate tap bpms for many of my music that was missing it.  It is a small tui-based music player that plays the selected song on loop, allowing you to tap the spacebar to generate the beats-per-minute of the song.  The BPMs are calculated as an avarage of the space between your last 10 taps.  It supports both MP3 and Flac tags.

## Usage

```
crabtap song1.mp3 song2.flac
```

## Controls

* **Space**: Tap to generate BPM data.
* **Enter**: Write BPM data to file (with confirmation prompt).
* **Up/K/Down/J**: Change songs.
* **R**: Restart current song
* **M**: To manually input a bpm
* **Esc/Q**: Quit

## Crabtapfilter

`crabtapfilter` is a helper binary used to filter out only songs that do not already have bpm data.

```
crabtapfilter *.{mp3,flac} | xargs -d '\n' crabtap
```

## License

See LICENSE for current license information
