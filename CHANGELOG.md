# Changelog

All notable changes to vortex-mod-youtube will be documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

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
