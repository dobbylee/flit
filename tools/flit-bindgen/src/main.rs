use std::{env, ffi::OsString, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use camino::Utf8PathBuf;
use flit_protocol::PROTOCOL_VERSION;
use uniffi_bindgen::bindings::{GenerateOptions, TargetLanguage, generate};

fn required_path(value: Option<OsString>, description: &str) -> Result<Utf8PathBuf> {
    let path = PathBuf::from(value.with_context(|| format!("missing {description}"))?);
    Utf8PathBuf::from_path_buf(path)
        .map_err(|path| anyhow::anyhow!("{description} is not valid UTF-8: {}", path.display()))
}

fn main() -> Result<()> {
    let mut arguments = env::args_os().skip(1);
    let library = required_path(arguments.next(), "compiled bridge library path")?;
    let output = required_path(arguments.next(), "binding output directory")?;
    if arguments.next().is_some() {
        bail!("expected exactly a compiled bridge library path and output directory");
    }

    generate(GenerateOptions {
        languages: vec![TargetLanguage::Swift],
        source: library,
        out_dir: output.clone(),
        config_override: None,
        format: false,
        crate_filter: Some("flit_bridge".to_owned()),
        metadata_no_deps: true,
    })
    .context("generating Swift bindings from the compiled flit-bridge metadata")?;

    if !PROTOCOL_VERSION
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_'))
    {
        bail!("protocol version contains characters unsupported by the Swift generator");
    }
    fs::write(
        output.join("FlitProtocol.swift"),
        format!(
            "// Generated from flit_protocol::PROTOCOL_VERSION. Do not edit.\n\
             let flitClientProtocolVersion = \"{PROTOCOL_VERSION}\"\n"
        ),
    )
    .context("writing the generated Swift client protocol version")
}
