use std::{fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    write_if_changed(
        &repository.join(flit_protocol::event_schema_relative_path()),
        &flit_protocol::generated_event_schema(),
    )?;
    Ok(())
}

fn write_if_changed(output: &Path, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
    if fs::read_to_string(output).ok().as_deref() == Some(contents) {
        return Ok(());
    }

    let parent = output
        .parent()
        .ok_or("generated protocol path has no parent")?;
    fs::create_dir_all(parent)?;
    fs::write(output, contents)?;
    Ok(())
}
