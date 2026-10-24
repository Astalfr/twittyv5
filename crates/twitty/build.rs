//! Embeds the app icon into `twitty.exe`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=resources/twitty.rc");
    println!("cargo:rerun-if-changed=resources/twitty.ico");
    // A no-op on other targets. Fails the build if the resource compiler is
    // missing, rather than silently shipping an exe without its icon.
    embed_resource::compile("resources/twitty.rc", embed_resource::NONE).manifest_required()?;
    Ok(())
}
