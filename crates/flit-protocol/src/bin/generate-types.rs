use std::{fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/generated/protocol.ts");
    let contents = flit_protocol::generated_typescript();

    if fs::read_to_string(&output).ok().as_deref() == Some(contents.as_str()) {
        return Ok(());
    }

    let parent = output
        .parent()
        .ok_or("generated protocol path has no parent")?;
    fs::create_dir_all(parent)?;
    fs::write(output, contents)?;
    Ok(())
}
