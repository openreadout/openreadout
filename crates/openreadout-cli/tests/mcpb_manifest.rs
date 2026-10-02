//! The `.mcpb` bundle manifest (`mcpb/manifest.json`, shown by Claude Desktop before install)
//! lists exactly the tools the MCP server offers.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

#[test]
fn mcpb_manifest_lists_every_mcp_tool() {
    let out = Command::new(env!("CARGO_BIN_EXE_openreadout"))
        .args(["self", "doctor", "--json"])
        .output()
        .unwrap();
    let doctor: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    if doctor["data"]["mcp"] != true {
        return; // built without the `mcp` feature: nothing to compare
    }
    let served: BTreeSet<String> = doctor["data"]["mcp_tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().to_string())
        .collect();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mcpb/manifest.json");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return; // outside the repository (e.g. a packaged crate)
    };
    let m: serde_json::Value = serde_json::from_str(&text).unwrap();
    let listed: BTreeSet<String> = m["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        served.difference(&listed).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "tools missing from mcpb/manifest.json"
    );
    assert_eq!(
        listed.difference(&served).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "mcpb/manifest.json lists tools the server does not offer"
    );
}
