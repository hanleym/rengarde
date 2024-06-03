use anyhow::Result;
use vergen::EmitBuilder;

pub fn main() -> Result<()> {
    EmitBuilder::builder()
        .all_build()
        .all_cargo()
        .all_git()
        .emit()?;
    Ok(())
}
