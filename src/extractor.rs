//! yt-dlp subprocess request/response helpers.
//!
//! The actual host-function call lives in `lib.rs`. This module provides
//! pure, unit-testable helpers to build the subprocess request and parse
//! the response.

use serde::{Deserialize, Serialize};

use crate::error::PluginError;

/// JSON request shape expected by the host's `run_subprocess` function.
#[derive(Debug, Serialize)]
pub struct SubprocessRequest {
    pub binary: String,
    pub args: Vec<String>,
    pub timeout_ms: u64,
}

/// JSON response shape returned by the host's `run_subprocess` function.
#[derive(Debug, Deserialize)]
pub struct SubprocessResponse {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Default subprocess timeout — 60 seconds.
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// Build the yt-dlp CLI arguments for a single video.
///
/// Uses `--dump-json` with `--no-playlist` to avoid accidentally expanding
/// playlists on mixed URLs (e.g. `watch?v=...&list=...`). A `--` sentinel is
/// inserted before the URL so that a URL accidentally starting with `-` can
/// never be interpreted as a yt-dlp option (defense in depth — URL is already
/// host-validated by [`crate::ensure_youtube_url`]).
pub fn yt_dlp_args_for_single_video(url: &str) -> Vec<String> {
    vec![
        "--dump-json".into(),
        "--no-playlist".into(),
        "--no-warnings".into(),
        "--".into(),
        url.into(),
    ]
}

/// Build the yt-dlp CLI arguments for flat playlist extraction.
pub fn yt_dlp_args_for_playlist(url: &str) -> Vec<String> {
    vec![
        "--dump-json".into(),
        "--flat-playlist".into(),
        "--no-warnings".into(),
        "--".into(),
        url.into(),
    ]
}

/// Build the yt-dlp CLI arguments to resolve a direct CDN stream URL.
///
/// Uses `--get-url` which instructs yt-dlp to print the final CDN URL(s)
/// instead of downloading. `--no-playlist` prevents accidental playlist
/// expansion on mixed URLs.
pub fn yt_dlp_args_for_stream_url(
    url: &str,
    quality: &str,
    format: &str,
    audio_only: bool,
) -> Vec<String> {
    let selector = build_format_selector(quality, format, audio_only);
    vec![
        "--get-url".into(),
        "--no-playlist".into(),
        "--no-warnings".into(),
        "--format".into(),
        selector,
        "--".into(),
        url.into(),
    ]
}

/// Build a yt-dlp format selector string from quality / format preferences.
///
/// Quality strings are accepted as either a bare number (`"720"`) or with
/// the trailing `p` suffix (`"720p"`). An empty or non-numeric quality
/// string is treated as unconstrained ("best"). The `format` string is
/// interpreted as a file extension constraint (e.g. `"mp4"`, `"webm"`).
/// Both quality and format are optional; an empty string disables the
/// respective constraint.
///
/// **Muxed-only**: uses the `best` yt-dlp format family, which selects a
/// pre-merged video+audio stream. This emits **one** CDN URL from
/// `--get-url`, which the Vortex download engine can fetch directly.
///
/// DASH formats (`bestvideo+bestaudio`) emit **two** URLs and require
/// ffmpeg for muxing — not yet supported by the Vortex core engine.
/// When the user requests a height where YouTube only offers DASH (>480p),
/// yt-dlp automatically falls back to the best available pre-muxed stream.
///
/// This is `pub` so that the format-selector logic can be unit-tested from
/// a native build without touching the WASM host-function layer.
pub fn build_format_selector(quality: &str, format: &str, audio_only: bool) -> String {
    let height: Option<u32> = quality.trim_end_matches('p').parse().ok();
    // Reject non-alphanumeric format strings (e.g. containing `]`, `/`, `+`)
    // that would produce an invalid yt-dlp selector. Fall back to no-format
    // constraint rather than passing a malformed selector to yt-dlp.
    let has_format =
        !format.is_empty() && format.chars().all(|c| c.is_ascii_alphanumeric());

    if audio_only {
        if has_format {
            format!("bestaudio[ext={format}]/bestaudio")
        } else {
            "bestaudio".into()
        }
    } else {
        match (height, has_format) {
            (Some(h), true) => format!("best[height<={h}][ext={format}]/best[height<={h}]/best"),
            (Some(h), false) => format!("best[height<={h}]/best"),
            (None, true) => format!("best[ext={format}]/best"),
            (None, false) => "best".into(),
        }
    }
}

/// Serialize a subprocess request as the JSON string expected by the host.
///
/// Returns [`PluginError::SerdeJson`] in the (practically unreachable) case
/// where serde cannot serialise a struct of plain `String` and `u64` fields.
/// The contract is enforced at compile time by the `Serialize` impl, but we
/// propagate the error rather than panic to honour the project's
/// zero-unwrap/expect policy.
pub fn build_subprocess_request(args: Vec<String>) -> Result<String, PluginError> {
    let req = SubprocessRequest {
        binary: "yt-dlp".into(),
        args,
        timeout_ms: DEFAULT_TIMEOUT_MS,
    };
    Ok(serde_json::to_string(&req)?)
}

/// Deserialize the host's subprocess response JSON and extract stdout.
///
/// Returns [`PluginError::Subprocess`] when the exit code is non-zero.
pub fn parse_subprocess_response(response_json: &str) -> Result<String, PluginError> {
    let resp: SubprocessResponse = serde_json::from_str(response_json)?;

    if resp.exit_code != 0 {
        return Err(PluginError::Subprocess {
            exit_code: resp.exit_code,
            stderr: truncate_stderr(&resp.stderr),
        });
    }

    Ok(resp.stdout)
}

/// Keep stderr manageable for error messages.
///
/// Truncation is performed on character boundaries, never byte offsets, so
/// multi-byte yt-dlp output (filenames with non-ASCII titles, localised
/// messages) cannot cause a panic. In WASM that would otherwise abort the
/// plugin instance without unwinding.
fn truncate_stderr(stderr: &str) -> String {
    const MAX_CHARS: usize = 512;
    let trimmed = stderr.trim();
    let char_count = trimmed.chars().count();
    if char_count <= MAX_CHARS {
        trimmed.to_string()
    } else {
        let mut out: String = trimmed.chars().take(MAX_CHARS).collect();
        out.push('…');
        out
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_video_args_include_no_playlist_flag() {
        let args = yt_dlp_args_for_single_video("https://youtu.be/abc12345678");
        assert!(args.contains(&"--dump-json".into()));
        assert!(args.contains(&"--no-playlist".into()));
        assert!(args.contains(&"https://youtu.be/abc12345678".into()));
    }

    #[test]
    fn playlist_args_include_flat_playlist_flag() {
        let args = yt_dlp_args_for_playlist("https://www.youtube.com/playlist?list=PLxyz");
        assert!(args.contains(&"--flat-playlist".into()));
        assert!(args.contains(&"--dump-json".into()));
    }

    #[test]
    fn subprocess_request_serialises_with_yt_dlp_binary() {
        let req_json = build_subprocess_request(vec!["--version".into()]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&req_json).unwrap();
        assert_eq!(parsed["binary"], "yt-dlp");
        assert_eq!(parsed["args"][0], "--version");
        assert_eq!(parsed["timeout_ms"], DEFAULT_TIMEOUT_MS);
    }

    #[test]
    fn parse_response_returns_stdout_on_success() {
        let json = r#"{"exit_code":0,"stdout":"ok","stderr":""}"#;
        assert_eq!(parse_subprocess_response(json).unwrap(), "ok");
    }

    #[test]
    fn parse_response_errors_on_non_zero_exit_code() {
        let json = r#"{"exit_code":1,"stdout":"","stderr":"ERROR: video unavailable"}"#;
        let result = parse_subprocess_response(json);
        match result {
            Err(PluginError::Subprocess { exit_code, stderr }) => {
                assert_eq!(exit_code, 1);
                assert!(stderr.contains("video unavailable"));
            }
            _ => panic!("expected Subprocess error, got {result:?}"),
        }
    }

    #[test]
    fn parse_response_errors_on_invalid_json() {
        let result = parse_subprocess_response("not json");
        assert!(matches!(result, Err(PluginError::SerdeJson(_))));
    }

    #[test]
    fn truncates_stderr_on_character_boundaries() {
        // Repeat a multi-byte grapheme past the character cap; ensure no panic
        // and that the truncation happens on a char boundary.
        let long = "é".repeat(2000);
        let json = format!(r#"{{"exit_code":1,"stdout":"","stderr":"{long}"}}"#);
        let result = parse_subprocess_response(&json);
        match result {
            Err(PluginError::Subprocess { stderr, .. }) => {
                // All chars are 'é' (2 bytes each); truncated to 512 + ellipsis
                assert!(stderr.chars().count() <= 513);
                assert!(stderr.ends_with('…'));
            }
            _ => panic!("expected Subprocess error"),
        }
    }

    #[test]
    fn build_request_includes_dash_dash_sentinel() {
        let args = yt_dlp_args_for_single_video("https://youtu.be/abc");
        let dash_idx = args.iter().position(|a| a == "--").expect("expected --");
        let url_idx = args
            .iter()
            .position(|a| a == "https://youtu.be/abc")
            .expect("expected url");
        assert!(dash_idx < url_idx);
    }

    #[test]
    fn build_format_selector_video_with_height_and_format() {
        assert_eq!(
            build_format_selector("720p", "mp4", false),
            "best[height<=720][ext=mp4]/best[height<=720]/best"
        );
    }

    #[test]
    fn build_format_selector_video_height_only() {
        assert_eq!(
            build_format_selector("1080", "", false),
            "best[height<=1080]/best"
        );
    }

    #[test]
    fn build_format_selector_video_unconstrained() {
        assert_eq!(build_format_selector("", "", false), "best");
    }

    #[test]
    fn build_format_selector_audio_with_format() {
        assert_eq!(
            build_format_selector("", "m4a", true),
            "bestaudio[ext=m4a]/bestaudio"
        );
    }

    #[test]
    fn build_format_selector_audio_unconstrained() {
        assert_eq!(build_format_selector("720p", "", true), "bestaudio");
    }

    #[test]
    fn build_format_selector_ignores_invalid_format_characters() {
        // Characters like `]`, `/`, `+` would break the yt-dlp selector syntax.
        // The function must treat them as if no format was specified.
        assert_eq!(
            build_format_selector("720p", "mp4/best", false),
            "best[height<=720]/best"
        );
        assert_eq!(
            build_format_selector("", "ext=mp4]", false),
            "best"
        );
    }

    #[test]
    fn stream_url_args_include_get_url_flag() {
        let args = yt_dlp_args_for_stream_url(
            "https://youtu.be/abc12345678",
            "720p",
            "mp4",
            false,
        );
        assert!(args.contains(&"--get-url".into()));
        assert!(args.contains(&"--no-playlist".into()));
        assert!(args.contains(&"--format".into()));
        let fmt_idx = args.iter().position(|a| a == "--format").unwrap();
        assert_eq!(
            args[fmt_idx + 1],
            "best[height<=720][ext=mp4]/best[height<=720]/best"
        );
    }

    #[test]
    fn truncates_long_stderr() {
        let long = "x".repeat(2000);
        let json = format!(r#"{{"exit_code":1,"stdout":"","stderr":"{long}"}}"#);
        let result = parse_subprocess_response(&json);
        match result {
            Err(PluginError::Subprocess { stderr, .. }) => {
                assert!(stderr.len() < 600);
                assert!(stderr.ends_with('…'));
            }
            _ => panic!("expected Subprocess error"),
        }
    }
}
