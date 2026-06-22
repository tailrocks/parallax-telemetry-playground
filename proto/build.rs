fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure().compile_protos(&["pricing.proto"], &["."])?;
    println!("cargo:rerun-if-changed=pricing.proto");
    Ok(())
}
