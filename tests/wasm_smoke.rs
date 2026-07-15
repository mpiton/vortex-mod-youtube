//! ABI smoke test for the release WASM artifact.

use std::path::PathBuf;

use extism::{Function, UserData, Val, PTR};

const WASM_REL_PATH: &str = "target/wasm32-wasip1/release/vortex_mod_youtube.wasm";

fn wasm_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(WASM_REL_PATH);
    assert!(
        path.is_file(),
        "missing release WASM artifact; run `cargo build --target wasm32-wasip1 --release` first"
    );
    path
}

fn stub_run_ytdlp() -> Function {
    Function::new(
        "run_ytdlp",
        [PTR],
        [PTR],
        UserData::<()>::default(),
        |plugin, inputs, outputs, _user_data: UserData<()>| {
            let input = inputs[0]
                .i64()
                .ok_or_else(|| extism::Error::msg("expected i64 input"))?;
            let request: String = plugin.memory_get_val(&Val::I64(input))?;
            let request: serde_json::Value = serde_json::from_str(&request)?;
            if ["binary", "args", "timeout_ms"]
                .iter()
                .any(|field| request.get(field).is_some())
            {
                return Err(extism::Error::msg("plugin exposed process controls"));
            }

            let stdout = match request["action"].as_str() {
                Some("metadata") if request["playlist"] == true => {
                    r#"{"id":"abc12345678","title":"First","url":"https://youtu.be/abc12345678","playlist_id":"PLtest","playlist":"Demo"}
{"id":"def12345678","title":"Second","url":"https://youtu.be/def12345678","playlist_id":"PLtest","playlist":"Demo"}
"#
                }
                Some("metadata") => {
                    r#"{"id":"abc12345678","title":"Demo","webpage_url":"https://youtu.be/abc12345678","duration":42,"formats":[{"format_id":"18","ext":"mp4","height":360,"width":640,"vcodec":"avc1","acodec":"mp4a"}]}"#
                }
                Some("resolve") => "https://cdn.example/video.mp4\n",
                Some("download") => "/tmp/vortex-downloads/job/video.mp4\n",
                _ => return Err(extism::Error::msg("unexpected yt-dlp action")),
            };
            let response = serde_json::json!({
                "exit_code": 0,
                "stdout": stdout,
                "stderr": ""
            })
            .to_string();
            let handle = plugin.memory_new(&response)?;
            outputs[0] = Val::I64(handle.offset() as i64);
            Ok(())
        },
    )
}

fn load_plugin() -> extism::Plugin {
    let manifest = extism::Manifest::new([extism::Wasm::file(wasm_path())]);
    extism::Plugin::new(&manifest, [stub_run_ytdlp()], true).expect("load YouTube WASM")
}

#[test]
fn wasm_routing_exports_are_callable() {
    let mut plugin = load_plugin();
    let can_handle: String = plugin
        .call("can_handle", "https://youtu.be/abc12345678")
        .expect("can_handle");
    let supports_playlist: String = plugin
        .call(
            "supports_playlist",
            "https://youtube.com/playlist?list=PLtest",
        )
        .expect("supports_playlist");

    assert_eq!(can_handle.trim(), "true");
    assert_eq!(supports_playlist.trim(), "true");
}

#[test]
fn wasm_metadata_exports_use_the_typed_broker() {
    let mut plugin = load_plugin();
    let links: String = plugin
        .call("extract_links", "https://youtu.be/abc12345678")
        .expect("extract_links");
    let variants: String = plugin
        .call("get_media_variants", "https://youtu.be/abc12345678")
        .expect("get_media_variants");
    let playlist: String = plugin
        .call(
            "extract_playlist",
            "https://youtube.com/playlist?list=PLtest",
        )
        .expect("extract_playlist");

    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&links).expect("links JSON")["kind"],
        "video"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&variants).expect("variants JSON")["variants"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&playlist).expect("playlist JSON")["videos"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
}

#[test]
fn wasm_media_exports_use_the_typed_broker() {
    let mut plugin = load_plugin();
    let resolve_input = r#"{"url":"https://youtu.be/abc12345678","quality":"480p","format":"mp4","audio_only":false}"#;
    let download_input = r#"{"url":"https://youtu.be/abc12345678","quality":"1080p","format":"mp4","output_dir":"/tmp/vortex-downloads/job","audio_only":false}"#;
    let direct_url: String = plugin
        .call("resolve_stream_url", resolve_input)
        .expect("resolve_stream_url");
    let path: String = plugin
        .call("download_to_file", download_input)
        .expect("download_to_file");

    assert_eq!(direct_url, "https://cdn.example/video.mp4");
    assert_eq!(path, "/tmp/vortex-downloads/job/video.mp4");
}
