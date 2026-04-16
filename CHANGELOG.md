# Changelog

All notable changes to vortex-mod-youtube will be documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [1.2.3] - 2026-04-16

### Fixed
- `download_to_file` returning `HTTP Error 403: Forbidden` on protected YouTube
  videos (VEVO music, age-gated content, some regional restrictions). Added
  `--extractor-args "youtube:player_client=default,web_safari,android_vr,tv"`
  to fall back across multiple YouTube player clients — `android_vr` and `tv`
  clients have weaker SABR/PO-token gating as of early 2026.
- yt-dlp progress output (tens of thousands of lines on long downloads) could
  saturate the host subprocess stderr pipe (1 MB cap in
  `host_functions.rs::MAX_SUBPROCESS_OUTPUT_BYTES`). Added `--quiet` so only
  the `after_move:%(filepath)s` line is emitted.

### Added
- `--retries 3` and `--fragment-retries 3` on the download args to recover
  from transient HTTP 5xx and dropped DASH fragments without aborting the
  whole download.

## [1.2.2] - 2026-04-16

### Fixed
- `resolve_stream_url` was still returning a 360p stream for 1080p requests
  because the format selector `best[height<=1080][protocol=https]` matched the
  360p pre-merged stream (360 ≤ 1080). Root cause: YouTube only provides
  pre-merged HTTPS streams at ≤480p. For 720p+ requests, `AdaptiveStreamOnly`
  is now returned immediately without calling yt-dlp, correctly routing to the
  `download_to_file` DASH+ffmpeg merge pipeline.

## [1.2.1] - 2026-04-16

### Fixed
- `resolve_stream_url` was silently returning a 360p stream instead of signalling
  `AdaptiveStreamOnly` when 1080p/720p was requested but unavailable as a
  pre-merged HTTPS stream. Root cause: the yt-dlp format selector included a
  height-unconstrained fallback (`/best[protocol=https]`) that always succeeded.
  Fallback removed for height-constrained requests; empty stdout now correctly
  maps to `AdaptiveStreamOnly`, triggering the DASH+ffmpeg merge pipeline.

## [1.2.0] - 2026-04-16

### Added
- `download_to_file` plugin function: delegates DASH download + ffmpeg merge to
  yt-dlp, enabling true 1080p/1440p/2160p downloads from YouTube.

### Fixed
- Downloads silently downgrading to 360p when 1080p was requested but only DASH
  streams were available (YouTube DASH-only qualities now supported).

## [1.1.1] - previous release

- Previous version (no changelog maintained).
