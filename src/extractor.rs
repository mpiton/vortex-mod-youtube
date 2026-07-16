//! Typed request/response helpers for the host-managed yt-dlp broker.

use serde::{Deserialize, Serialize};

use crate::error::PluginError;

/// Closed request contract accepted by Vortex's `run_ytdlp` host function.
///
/// Process selection, command-line arguments, timeouts, environment, and the
/// working directory are deliberately absent: those controls belong to the
/// trusted host.
#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum YtDlpRequest<'a> {
    Metadata {
        url: &'a str,
        playlist: bool,
    },
    Resolve {
        url: &'a str,
        quality: Option<u32>,
        format: Option<&'a str>,
        audio_only: bool,
    },
    Download {
        url: &'a str,
        quality: Option<u32>,
        format: Option<&'a str>,
        output_dir: &'a str,
        audio_only: bool,
    },
}

#[derive(Debug, Deserialize)]
struct YtDlpResponse {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

pub fn build_metadata_request(url: &str, playlist: bool) -> Result<String, PluginError> {
    serialize_request(YtDlpRequest::Metadata { url, playlist })
}

pub fn build_resolve_request(
    url: &str,
    quality: &str,
    format: &str,
    audio_only: bool,
) -> Result<String, PluginError> {
    serialize_request(YtDlpRequest::Resolve {
        url,
        quality: parse_quality(quality),
        format: optional_format(format),
        audio_only,
    })
}

pub fn build_download_request(
    url: &str,
    quality: &str,
    format: &str,
    output_dir: &str,
    audio_only: bool,
) -> Result<String, PluginError> {
    serialize_request(YtDlpRequest::Download {
        url,
        quality: parse_quality(quality),
        format: optional_format(format),
        output_dir,
        audio_only,
    })
}

fn serialize_request(request: YtDlpRequest<'_>) -> Result<String, PluginError> {
    Ok(serde_json::to_string(&request)?)
}

fn parse_quality(quality: &str) -> Option<u32> {
    let quality = quality.trim();
    if quality.is_empty() || quality.eq_ignore_ascii_case("best") {
        return None;
    }
    quality.trim_end_matches('p').parse().ok()
}

fn optional_format(format: &str) -> Option<&str> {
    let format = format.trim();
    (!format.is_empty()).then_some(format)
}

/// Deserialize the broker response and extract stdout.
pub fn parse_ytdlp_response(response_json: &str) -> Result<String, PluginError> {
    let response: YtDlpResponse = serde_json::from_str(response_json)?;
    if response.exit_code != 0 {
        return Err(PluginError::Subprocess {
            exit_code: response.exit_code,
            stderr: truncate_stderr(&response.stderr),
        });
    }
    Ok(response.stdout)
}

/// Parse the final merged file path printed by the host-managed download.
pub fn parse_download_path_from_stdout(stdout: &str) -> Result<String, PluginError> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .ok_or(PluginError::NoMatchingFormat)
}

fn truncate_stderr(stderr: &str) -> String {
    const MAX_CHARS: usize = 512;
    let trimmed = stderr.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        return trimmed.to_string();
    }
    let mut output: String = trimmed.chars().take(MAX_CHARS).collect();
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_metadata_request_exposes_no_process_controls() {
        let json = build_metadata_request("https://youtu.be/abc12345678", false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["action"], "metadata");
        assert_eq!(value["playlist"], false);
        assert!(value.get("binary").is_none());
        assert!(value.get("args").is_none());
        assert!(value.get("timeout_ms").is_none());
    }

    #[test]
    fn typed_playlist_request_sets_playlist_flag() {
        let json = build_metadata_request("https://youtube.com/playlist?list=PLxyz", true).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["action"], "metadata");
        assert_eq!(value["playlist"], true);
    }

    #[test]
    fn typed_resolve_request_carries_preferences_not_arguments() {
        let json =
            build_resolve_request("https://youtu.be/abc12345678", "480p", "mp4", false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["action"], "resolve");
        assert_eq!(value["quality"], 480);
        assert_eq!(value["format"], "mp4");
        assert!(value.get("args").is_none());
    }

    #[test]
    fn best_and_empty_preferences_are_serialized_as_null() {
        let json =
            build_resolve_request("https://youtu.be/abc12345678", "best", "", false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(value["quality"].is_null());
        assert!(value["format"].is_null());
    }

    #[test]
    fn typed_download_request_carries_host_validated_output_directory() {
        let json = build_download_request(
            "https://youtu.be/abc12345678",
            "1080p",
            "mkv",
            "/tmp/vortex-downloads/job",
            false,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["action"], "download");
        assert_eq!(value["quality"], 1080);
        assert_eq!(value["output_dir"], "/tmp/vortex-downloads/job");
        assert!(value.get("binary").is_none());
        assert!(value.get("args").is_none());
    }

    #[test]
    fn parse_response_returns_stdout_on_success() {
        let json = r#"{"exit_code":0,"stdout":"ok","stderr":""}"#;
        assert_eq!(parse_ytdlp_response(json).unwrap(), "ok");
    }

    #[test]
    fn parse_response_errors_on_non_zero_exit_code() {
        let json = r#"{"exit_code":1,"stdout":"","stderr":"ERROR: video unavailable"}"#;
        let result = parse_ytdlp_response(json);

        match result {
            Err(PluginError::Subprocess { exit_code, stderr }) => {
                assert_eq!(exit_code, 1);
                assert!(stderr.contains("video unavailable"));
            }
            _ => panic!("expected Subprocess error, got {result:?}"),
        }
    }

    #[test]
    fn truncates_stderr_on_character_boundaries() {
        let long = "é".repeat(2000);
        let json = format!(r#"{{"exit_code":1,"stdout":"","stderr":"{long}"}}"#);
        let result = parse_ytdlp_response(&json);

        match result {
            Err(PluginError::Subprocess { stderr, .. }) => {
                assert_eq!(stderr.chars().count(), 513);
                assert!(stderr.ends_with('…'));
            }
            _ => panic!("expected Subprocess error"),
        }
    }

    #[test]
    fn parse_download_path_returns_last_nonempty_line() {
        let stdout = "\n/tmp/vortex-downloads/job/video.mp4\n";
        assert_eq!(
            parse_download_path_from_stdout(stdout).unwrap(),
            "/tmp/vortex-downloads/job/video.mp4"
        );
    }

    #[test]
    fn parse_download_path_empty_stdout_returns_error() {
        assert!(matches!(
            parse_download_path_from_stdout("   \n  \n"),
            Err(PluginError::NoMatchingFormat)
        ));
    }
}
