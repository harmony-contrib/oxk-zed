use std::{collections::BTreeSet, env, fs};
use wasmparser::{Parser, Payload, Validator};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .unwrap_or_else(|| "extension.wasm".into());
    let bytes = fs::read(&path)?;
    Validator::new().validate_all(&bytes)?;
    let mut api = None;
    let mut exports = BTreeSet::new();
    for payload in Parser::new(0).parse_all(&bytes) {
        if let Payload::CustomSection(section) = payload? {
            if section.name() == "zed:api-version" {
                let data = section.data();
                if data.len() != 6 {
                    return Err("invalid Zed API version metadata".into());
                }
                api = Some((
                    u16::from_be_bytes([data[0], data[1]]),
                    u16::from_be_bytes([data[2], data[3]]),
                    u16::from_be_bytes([data[4], data[5]]),
                ));
            }
        }
    }
    for payload in Parser::new(0).parse_all(&bytes) {
        if let Payload::ComponentExportSection(section) = payload? {
            for export in section {
                exports.insert(export?.name.0.to_string());
            }
        }
    }
    if api != Some((0, 7, 0)) {
        return Err(format!("unexpected Zed API version: {api:?}").into());
    }
    for required in [
        "init-extension",
        "language-server-command",
        "language-server-initialization-options",
        "language-server-workspace-configuration",
    ] {
        if !exports.contains(required) {
            return Err(format!("missing Zed component export: {required}").into());
        }
    }
    println!(
        "{path}: validated WASM and required Zed exports; API 0.7.0; {} bytes",
        bytes.len()
    );
    Ok(())
}
