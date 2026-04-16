//! WASM-only module: `#[plugin_fn]` exports and `#[host_fn]` imports.
//!
//! This module is gated behind `cfg(target_family = "wasm")` because the
//! macros emit code that only compiles for a WASM target (e.g. `cdylib`
//! exports, `extern "ExtismHost"` linkage). Pure logic lives in sibling
//! modules so that it can be unit-tested natively.

use extism_pdk::*;

use crate::error::PluginError;
use crate::extractor::{
    build_subprocess_request, parse_download_path_from_stdout, parse_subprocess_response,
    yt_dlp_args_for_download_to_file, yt_dlp_args_for_playlist, yt_dlp_args_for_single_video,
    yt_dlp_args_for_stream_url, DEFAULT_DOWNLOAD_TIMEOUT_MS,
};
use crate::metadata::{parse_flat_playlist, parse_single_video};
use crate::url_matcher::UrlKind;
use crate::{
    build_media_variants_response, build_playlist_response, build_single_video_response,
    ensure_playlist_or_channel, ensure_single_video, ensure_youtube_url, handle_can_handle,
    handle_supports_playlist,
};

// ── Host function imports ─────────────────────────────────────────────────────

#[host_fn]
extern "ExtismHost" {
    /// JSON in → JSON out — see `SubprocessRequest` / `SubprocessResponse`.
    fn run_subprocess(req: String) -> String;
}

// ── Plugin function exports ───────────────────────────────────────────────────

/// Returns `"true"` if the URL is any form of recognised YouTube resource.
#[plugin_fn]
pub fn can_handle(url: String) -> FnResult<String> {
    Ok(handle_can_handle(&url))
}

/// Returns `"true"` if the URL refers to a playlist or channel.
#[plugin_fn]
pub fn supports_playlist(url: String) -> FnResult<String> {
    Ok(handle_supports_playlist(&url))
}

/// Extract media links from a single video or playlist URL.
///
/// Dispatches to `yt-dlp --dump-json` (single video) or
/// `yt-dlp --dump-json --flat-playlist` (playlist / channel).
#[plugin_fn]
pub fn extract_links(url: String) -> FnResult<String> {
    let kind = ensure_youtube_url(&url).map_err(error_to_fn_error)?;

    let response = match kind {
        UrlKind::Playlist | UrlKind::Channel => {
            let stdout = call_yt_dlp(yt_dlp_args_for_playlist(&url))?;
            let playlist = parse_flat_playlist(&stdout).map_err(error_to_fn_error)?;
            build_playlist_response(playlist)
        }
        UrlKind::Video | UrlKind::Shorts => {
            let stdout = call_yt_dlp(yt_dlp_args_for_single_video(&url))?;
            let video = parse_single_video(&stdout).map_err(error_to_fn_error)?;
            build_single_video_response(video)
        }
        // `ensure_youtube_url` rejects `Unknown` — this arm is unreachable,
        // but exhaustiveness matching forces a decision if a new kind is
        // added later. Return `UnsupportedUrl` for safety.
        UrlKind::Unknown => {
            return Err(error_to_fn_error(PluginError::UnsupportedUrl(url)));
        }
    };

    Ok(serde_json::to_string(&response)?)
}

/// List available media formats for a single video URL.
///
/// Rejects playlist / channel URLs explicitly — without this guard, yt-dlp
/// would silently extract the first video in the playlist (because the
/// args include `--no-playlist`) and return its variants as if they
/// belonged to the collection itself.
#[plugin_fn]
pub fn get_media_variants(url: String) -> FnResult<String> {
    ensure_single_video(&url).map_err(error_to_fn_error)?;

    let stdout = call_yt_dlp(yt_dlp_args_for_single_video(&url))?;
    let video = parse_single_video(&stdout).map_err(error_to_fn_error)?;
    let variants = build_media_variants_response(video);
    Ok(serde_json::to_string(&variants)?)
}

/// Extract a flat playlist listing.
///
/// Rejects single-video URLs explicitly so that callers get a clear
/// `UnsupportedUrl` error instead of yt-dlp falling back to single-item
/// extraction behaviour on a `watch?v=...` URL.
#[plugin_fn]
pub fn extract_playlist(url: String) -> FnResult<String> {
    ensure_playlist_or_channel(&url).map_err(error_to_fn_error)?;

    let stdout = call_yt_dlp(yt_dlp_args_for_playlist(&url))?;
    let playlist = parse_flat_playlist(&stdout).map_err(error_to_fn_error)?;
    let response = build_playlist_response(playlist);
    Ok(serde_json::to_string(&response)?)
}

/// Resolve the direct CDN stream URL for a single video with quality/format
/// preferences.
///
/// Input is a JSON object `{ "url", "quality"?, "format"?, "audio_only"? }`.
/// Returns the first non-empty CDN URL emitted by yt-dlp `--get-url`.
///
/// The format selector uses `best[protocol=https]` to guarantee a single
/// direct HTTPS URL that the Vortex download engine can fetch without any
/// adaptive-streaming logic. HLS (`m3u8_native`) and DASH (`http_dash_segments`)
/// formats are excluded. YouTube typically provides direct streams at ≤480p;
/// higher resolutions may not find a matching format.
///
/// Returns [`PluginError::NoMatchingFormat`] when yt-dlp emits no URLs at all,
/// and [`PluginError::AdaptiveStreamOnly`] if an HLS URL slips through.
#[plugin_fn]
pub fn resolve_stream_url(input: String) -> FnResult<String> {
    #[derive(serde::Deserialize)]
    struct Input {
        url: String,
        #[serde(default)]
        quality: String,
        #[serde(default)]
        format: String,
        #[serde(default)]
        audio_only: bool,
    }

    let params: Input =
        serde_json::from_str(&input).map_err(|e| error_to_fn_error(PluginError::SerdeJson(e)))?;

    ensure_single_video(&params.url).map_err(error_to_fn_error)?;

    // YouTube only provides pre-merged HTTPS streams at ≤480p (itag 18/36).
    // 720p and above are DASH-only and must go through download_to_file.
    // Signal AdaptiveStreamOnly immediately rather than letting yt-dlp silently
    // fall back to a lower-quality pre-merged stream.
    let requested_height: Option<u32> = params.quality.trim_end_matches('p').parse().ok();
    if requested_height.is_some_and(|h| h >= 720) {
        return Err(error_to_fn_error(PluginError::AdaptiveStreamOnly));
    }

    let stdout = call_yt_dlp(yt_dlp_args_for_stream_url(
        &params.url,
        &params.quality,
        &params.format,
        params.audio_only,
    ))?;

    // The direct-only selector should emit exactly one HTTPS URL. Take the
    // first non-empty line as a defensive measure against edge-case output.
    //
    // When a quality was explicitly requested and yt-dlp emits nothing, the
    // requested resolution is only available as DASH streams — signal
    // AdaptiveStreamOnly so Vortex core can delegate to download_to_file.
    let cdn_url = stdout
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| {
            if !params.quality.is_empty() {
                error_to_fn_error(PluginError::AdaptiveStreamOnly)
            } else {
                error_to_fn_error(PluginError::NoMatchingFormat)
            }
        })?
        .to_string();

    // Safety net: reject HLS/DASH URLs that slipped through the format
    // selector. The Vortex download engine requires a single direct HTTPS URL.
    if cdn_url.contains(".m3u8") || cdn_url.contains("manifest.googlevideo.com") {
        return Err(error_to_fn_error(PluginError::AdaptiveStreamOnly));
    }

    Ok(cdn_url)
}

/// Download a video/audio file using yt-dlp's native download+merge pipeline.
///
/// Use this when `resolve_stream_url` returns `AdaptiveStreamOnly` — i.e. when
/// the requested quality is only available as DASH streams that must be merged
/// with ffmpeg. yt-dlp handles the multi-stream download and ffmpeg merge
/// internally; the merged file is written to `output_dir` and its path is
/// returned as a raw string.
///
/// Input:  JSON `{ "url", "quality"?, "format"?, "output_dir", "audio_only"? }`
/// Output: absolute path of the merged file (raw string)
#[plugin_fn]
pub fn download_to_file(input: String) -> FnResult<String> {
    #[derive(serde::Deserialize)]
    struct Input {
        url: String,
        #[serde(default)]
        quality: String,
        #[serde(default)]
        format: String,
        output_dir: String,
        #[serde(default)]
        audio_only: bool,
    }

    let params: Input =
        serde_json::from_str(&input).map_err(|e| error_to_fn_error(PluginError::SerdeJson(e)))?;

    ensure_single_video(&params.url).map_err(error_to_fn_error)?;

    let args = yt_dlp_args_for_download_to_file(
        &params.url,
        &params.quality,
        &params.format,
        &params.output_dir,
        params.audio_only,
    );

    // Override timeout: full download+merge can take 30+ minutes.
    let req = crate::extractor::SubprocessRequest {
        binary: "yt-dlp".into(),
        args,
        timeout_ms: DEFAULT_DOWNLOAD_TIMEOUT_MS,
    };
    let req_json = serde_json::to_string(&req)
        .map_err(|e| error_to_fn_error(PluginError::SerdeJson(e)))?;

    let resp_json = unsafe { run_subprocess(req_json)? };
    let stdout = parse_subprocess_response(&resp_json).map_err(error_to_fn_error)?;

    parse_download_path_from_stdout(&stdout).map_err(error_to_fn_error)
}

// ── Host function wiring ──────────────────────────────────────────────────────

fn call_yt_dlp(args: Vec<String>) -> FnResult<String> {
    let req_json = build_subprocess_request(args).map_err(error_to_fn_error)?;
    // SAFETY: `run_subprocess` is resolved by the Vortex plugin host at
    // load time (see src-tauri/src/adapters/driven/plugin/host_functions.rs:
    // `make_run_subprocess_function`). Invariants:
    //   1. The host registers the symbol `run_subprocess` in the
    //      `ExtismHost` namespace before any `#[plugin_fn]` export is
    //      callable — a missing symbol would abort `Plugin::new` in
    //      extism_loader.rs and prevent the plugin from being loaded.
    //   2. The ABI is `(I64) -> I64` — a single u64 Extism memory
    //      handle in, a single u64 handle out. The `#[host_fn]` macro
    //      generates the correct marshalling from `String` to/from the
    //      memory handle.
    //   3. Host-side capability check rejects calls when the plugin
    //      manifest does not declare `subprocess:yt-dlp`; the host
    //      returns an error, which the `?` below propagates safely.
    //   4. `run_subprocess` has no aliasing or mutability concerns —
    //      inputs and outputs are owned, serialisable JSON strings.
    let resp_json = unsafe { run_subprocess(req_json)? };
    parse_subprocess_response(&resp_json).map_err(error_to_fn_error)
}

fn error_to_fn_error(err: PluginError) -> WithReturnCode<extism_pdk::Error> {
    extism_pdk::Error::msg(err.to_string()).into()
}
