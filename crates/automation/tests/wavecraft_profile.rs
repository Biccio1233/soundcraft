//! The WaveCraft profile: the same server and tools, under WaveCraft's name.

use serde_json::{Value, json};
use soundcraft_automation::Headless;
use soundcraft_automation::mcp::{SOUNDCRAFT, Server, WAVECRAFT};
use soundcraft_engine::Engine;

fn call(s: &mut Server<Headless>, id: u64, method: &str) -> Value {
    s.handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {"protocolVersion": "2025-06-18"}})).unwrap()["result"].clone()
}

#[test]
fn wavecraft_profile_names_the_server() {
    let mut s = Server::with_profile(Headless::new(Engine::default()), WAVECRAFT);
    let init = call(&mut s, 1, "initialize");
    assert_eq!(init["serverInfo"]["name"], "wavecraft");
    assert!(init["instructions"].as_str().unwrap().starts_with("WaveCraft"), "{init}");
    let uris: Vec<String> =
        call(&mut s, 2, "resources/list")["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap().to_string()).collect();
    assert!(uris.iter().all(|u| u.starts_with("wavecraft://")), "{uris:?}");
    let tools = call(&mut s, 3, "tools/list").to_string();
    assert!(tools.contains("WaveCraft") && !tools.contains("SoundCraft"), "{tools}");
}

#[test]
fn the_default_profile_is_still_soundcraft() {
    let mut s = Server::new(Headless::new(Engine::default()));
    assert_eq!(call(&mut s, 1, "initialize")["serverInfo"]["name"], "soundcraft");
    assert_eq!(SOUNDCRAFT.name, "soundcraft");
}
